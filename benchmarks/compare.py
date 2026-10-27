"""Standalone HTTP probe: standard library only, no Atlas runtime imports."""

import argparse
import hashlib
import http.client
import json
import math
import platform
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime
from pathlib import Path
from time import perf_counter
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

MAX_BYTES = 32 * 1024 * 1024


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def normalize(payload, kind):
    if kind == "bytes":
        return payload
    # Preserve numeric spelling/precision and distinguish numbers from strings and booleans.
    return json.loads(payload, parse_int=lambda n: ("int", n), parse_float=lambda n: ("float", n))


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
            sample["outcome"] = "valid" if normalize(payload, kind) == expected else "invalid"
    except (OSError, URLError, http.client.HTTPException, ValueError) as error:
        sample["error_type"] = type(error).__name__
    sample.setdefault("ms", (perf_counter() - start) * 1000)
    return sample


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
    parser.add_argument("--kind", choices=("json", "bytes"), default="json")
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
        if key not in ("body", "expected")
    }
    metadata.update(
        expected_sha256=hashlib.sha256(raw).hexdigest(),
        body_sha256=hashlib.sha256(body).hexdigest() if body is not None else None,
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
            record(dict(type="summary", finished=finished, samples=len(samples), **summary))
    return 0 if finished and summary["counts"]["valid"] == args.samples else 1


if __name__ == "__main__":
    raise SystemExit(main())
