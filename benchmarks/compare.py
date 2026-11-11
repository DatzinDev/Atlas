"""Standalone HTTP probe: standard library only, no Atlas runtime imports."""

import argparse
import hashlib
import http.client
import json
import math
import platform
import re
import struct
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime
from pathlib import Path
from time import perf_counter
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

MAX_BYTES = 32 * 1024 * 1024
# Canonical cases mapped to the Rust REST API (docs/rust/api.md): method, kind, path suffix.
# ponytail: configuration only; another engine gets its own table when it is measured.
RUST_CASES = {
    "catalog": ("GET", "json", r"/datasets"),
    "geojson-page": ("GET", "json", r"/datasets/[^/]+/items"),
    "geojson-query": ("POST", "json", r"/datasets/[^/]+/query"),
    "mvt-tile": ("GET", "mvt", r"/datasets/[^/]+/tiles/\d+/\d+/\d+\.mvt"),
    "tilejson": ("GET", "json", r"/datasets/[^/]+/tilejson\.json"),
    "export": ("GET", "json", r"/datasets/[^/]+/export"),
}


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def varint(buf, i):
    shift = value = 0
    while i < len(buf) and shift < 64:
        value |= (buf[i] & 0x7F) << shift
        i, shift = i + 1, shift + 7
        if buf[i - 1] < 0x80:
            return value, i
    raise ValueError("truncated varint")


def fields(buf):
    """Yield (field, wire type, value) from a protobuf message; raise ValueError if malformed."""
    i = 0
    while i < len(buf):
        key, i = varint(buf, i)
        wire, size = key & 7, {1: 8, 5: 4}.get(key & 7)
        if wire == 0:
            value, i = varint(buf, i)
            yield key >> 3, wire, value
            continue
        if wire == 2:
            size, i = varint(buf, i)
        if size is None or i + size > len(buf):
            raise ValueError("malformed protobuf")
        yield key >> 3, wire, buf[i : i + size]
        i += size


def packed(buf):
    values, i = [], 0
    while i < len(buf):
        value, i = varint(buf, i)
        values.append(value)
    return values


def scalar(field, value):
    if field == 1:
        return "string", value.decode()
    if field in (2, 3):
        return ("float", "double")[field - 2], struct.unpack("<fd"[field - 2], value)[0]
    if field in (4, 5, 6):
        signed = value - (1 << 64) if value >= 1 << 63 else value
        return "int", (signed, value, (value >> 1) ^ -(value & 1))[field - 4]
    return "bool", bool(value)


def mvt(payload):
    """Decode a tile ignoring encoder order; counts, ids and quantized geometry stay exact."""
    layers = []
    try:
        for _, _, layer in (f for f in fields(payload) if f[0] == 3):
            parts = {}
            for field, _, value in fields(layer):
                parts.setdefault(field, []).append(value)
            keys = [key.decode() for key in parts.get(3, [])]
            values = [scalar(f, v) for raw in parts.get(4, []) for f, _, v in fields(raw)]
            features = []
            for raw in parts.get(2, []):
                feature = {field: value for field, _, value in fields(raw)}
                tags = packed(feature.get(2, b""))  # odd tag counts fail zip(strict=True)
                props = sorted(
                    (keys[k], values[v]) for k, v in zip(tags[::2], tags[1::2], strict=True)
                )
                geometry = tuple(packed(feature.get(4, b"")))
                features.append((feature.get(1), feature.get(3, 0), geometry, tuple(props)))
            name, extent = parts[1][0].decode(), parts.get(5, [4096])[0]
            layers.append((name, extent, sorted(features, key=repr)))
    except (AttributeError, IndexError, KeyError, TypeError, struct.error) as error:
        raise ValueError("malformed MVT") from error
    return sorted(layers, key=repr)


def normalize(payload, kind):
    if kind == "bytes":
        return payload
    if kind == "mvt":
        return mvt(payload)
    # Preserve numeric spelling/precision and distinguish numbers from strings and booleans.
    return json.loads(payload, parse_int=lambda n: ("int", n), parse_float=lambda n: ("float", n))


def mismatch(expected, actual, kind):
    """Name the first difference so count/id changes are never reported as plain content."""

    def ids(value):
        if kind == "mvt":
            return sorted(repr(f[0]) for layer in value for f in layer[2])
        features = value.get("features") if isinstance(value, dict) else None
        if isinstance(features, list):
            return sorted(repr(f.get("id") if isinstance(f, dict) else f) for f in features)
        return None

    want, got = ids(expected), ids(actual)
    if want is None or got is None:
        return "content"
    if len(want) != len(got):
        return "count"
    return "ids" if want != got else "geometry_or_properties"


def request_sample(url, expected, kind, timeout, body=None):
    sample = dict(status=0, bytes=0, outcome="error")
    start = perf_counter()
    try:
        request = Request(url, data=body, headers={"Content-Type": "application/json"})
        try:
            response = build_opener(NoRedirect).open(request, timeout=timeout)
        except HTTPError as error:
            response = error
        with response:
            sample["status"] = response.code
            payload = response.read(MAX_BYTES + 1)
        sample["ms"] = (perf_counter() - start) * 1000
        sample["bytes"] = len(payload)
        if len(payload) > MAX_BYTES:
            sample["outcome"] = "invalid"
        elif response.code in (413, 429, 503):
            sample["outcome"] = "rejected"
        elif response.code == 200:
            sample["outcome"] = "invalid"
            actual = normalize(payload, kind)
            if actual == expected:
                sample["outcome"] = "valid"
            else:
                sample["mismatch"] = mismatch(expected, actual, kind)
        if response.code != 200 and payload[:1] == b"{":
            sample["code"] = json.loads(payload).get("code")
    except (OSError, URLError, http.client.HTTPException, ValueError) as error:
        sample["error_type"] = type(error).__name__
    sample.setdefault("ms", (perf_counter() - start) * 1000)
    return sample


def cgroup(path):
    """Snapshot a cgroup v2 directory: server and database are reported separately."""
    cpu = dict(line.split() for line in (path / "cpu.stat").read_text().splitlines())
    peak = path / "memory.peak"
    return dict(
        cpu_seconds=int(cpu["usage_usec"]) / 1e6,
        memory_bytes=int((path / "memory.current").read_text()),
        memory_peak_bytes=int(peak.read_text()) if peak.exists() else None,
    )


def check_case(args):
    if args.engine != "rust":
        return None
    method, kind, suffix = RUST_CASES.get(args.case, (None, None, None))
    if suffix is None:
        return f"rust case must be one of {sorted(RUST_CASES)}"
    if not re.fullmatch(r"(/[^?#]*)?/api/v1" + suffix, urlsplit(args.url).path):
        return f"URL path does not match the Rust route for {args.case}"
    if (args.body is not None) != (method == "POST") or args.kind != kind:
        return f"{args.case} requires {method} with --kind {kind}"
    return None


def summarize(samples, elapsed):
    valid = [s for s in samples if s["outcome"] == "valid"]
    times = sorted(s["ms"] for s in valid)
    return dict(
        counts={
            key: sum(s["outcome"] == key for s in samples)
            for key in ("valid", "invalid", "rejected", "error")
        },
        p50_ms=times[math.ceil(len(times) * 0.50) - 1] if times else None,
        p95_ms=times[math.ceil(len(times) * 0.95) - 1] if times else None,
        valid_requests_per_second=len(valid) / elapsed if elapsed else 0,
        valid_bytes=sum(s["bytes"] for s in valid),
        wall_seconds=elapsed,
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("url", "engine", "revision", "dataset", "case"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--expected", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--body", type=Path, help="JSON file; enables POST instead of GET")
    parser.add_argument("--kind", choices=("json", "bytes", "mvt"), default="json")
    parser.add_argument("--pin", type=Path, action="append", default=[], help="file to hash")
    for name in ("server", "db"):
        parser.add_argument(f"--{name}-cgroup", type=Path, help="cgroup v2 directory")
    parser.add_argument("--samples", type=int, default=100)
    parser.add_argument("--warmup", type=int, default=10)
    parser.add_argument("--concurrency", type=int, default=1)
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args(argv)
    url = urlsplit(args.url)
    if url.scheme not in ("http", "https") or not url.hostname or url.username or url.password:
        parser.error("use an HTTP(S) URL without credentials")
    if not (
        1 <= args.samples <= 100000
        and 0 <= args.warmup <= 10000
        and 1 <= args.concurrency <= 32
        and 0 < args.timeout <= 300
    ):
        parser.error("invalid load budget")
    if problem := check_case(args):
        parser.error(problem)
    groups = {"server": args.server_cgroup, "db": args.db_cgroup}
    groups = {name: path for name, path in groups.items() if path is not None}
    raw = args.expected.read_bytes()
    if len(raw) > MAX_BYTES:
        parser.error("expected body exceeds 32 MiB")
    expected = normalize(raw, args.kind)
    body = args.body.read_bytes() if args.body else None
    if body is not None:
        json.loads(body)
    metadata = {
        key: str(value) if isinstance(value, Path) else value
        for key, value in vars(args).items()
        if key not in ("body", "expected", "pin")
    }
    metadata.update(
        expected_sha256=hashlib.sha256(raw).hexdigest(),
        body_sha256=hashlib.sha256(body).hexdigest() if body is not None else None,
        pins={str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in args.pin},
        resources_before={name: cgroup(path) for name, path in groups.items()},
        script_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        client_python=platform.python_version(),
        host=platform.platform(),
        started_at=datetime.now(UTC).isoformat(),
        finished=False,
    )
    samples = []
    finished = False
    start = None
    with args.output.open("x", encoding="utf-8") as output:

        def record(value):
            output.write(json.dumps(value, allow_nan=False) + "\n")
            output.flush()

        def call(_):
            return request_sample(args.url, expected, args.kind, args.timeout, body)

        record(dict(type="metadata", **metadata))
        try:
            for i in range(args.warmup):
                sample = call(i)
                record(dict(type="warmup", **sample))
                if sample["outcome"] != "valid":
                    return 1
            start = perf_counter()
            with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
                for sample in pool.map(call, range(args.samples)):
                    samples.append(sample)
                    record(dict(type="sample", **sample))
            finished = True
        finally:
            summary = summarize(samples, perf_counter() - start if start is not None else 0)
            try:
                summary["resources"] = {name: cgroup(path) for name, path in groups.items()}
            except OSError:  # a cgroup vanished (e.g. container stopped): still write the summary
                summary["resources"] = None
            record(dict(type="summary", finished=finished, samples=len(samples), **summary))
    return 0 if finished and summary["counts"]["valid"] == args.samples else 1


if __name__ == "__main__":
    raise SystemExit(main())
