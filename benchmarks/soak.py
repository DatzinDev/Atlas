"""Soak: a seeded weighted mix of cases at fixed concurrency for a duration, one JSONL."""

import argparse
import hashlib
import json
import platform
import random
import sys
from datetime import UTC, datetime
from pathlib import Path
from time import perf_counter

from benchmarks.compare import cgroup, drive, normalize, request_sample, summarize


def load(entry):
    raw = Path(entry["expected"]).read_bytes()
    body = Path(entry["body"]).read_bytes() if entry.get("body") else None
    kind = entry.get("kind", "json")
    return dict(
        case=entry["case"],
        url=entry["url"],
        kind=kind,
        weight=entry["weight"],
        body=body,
        expected=normalize(raw, kind),
        expected_sha256=hashlib.sha256(raw).hexdigest(),
    )


def pick(mix, seed, index):
    return random.Random(f"{seed}:{index}").choices(mix, [m["weight"] for m in mix])[0]


def main(args=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    options = parser.parse_args(args)
    plan = json.loads(options.plan.read_text())
    mix = [load(entry) for entry in plan["mix"]]
    if not mix or any(m["weight"] <= 0 for m in mix) or not 0 < plan["duration"] <= 7200:
        parser.error("invalid mix or duration")
    timeout, seed = plan.get("timeout", 30), plan.get("seed", 0)
    groups = {n: Path(plan[f"{n}_cgroup"]) for n in ("server", "db") if plan.get(f"{n}_cgroup")}
    metadata = {k: v for k, v in plan.items() if k != "mix"}
    metadata.update(
        mix=[{k: m[k] for k in ("case", "url", "kind", "weight", "expected_sha256")} for m in mix],
        plan_sha256=hashlib.sha256(options.plan.read_bytes()).hexdigest(),
        resources_before={name: cgroup(path) for name, path in groups.items()},
        script_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        client_python=platform.python_version(),
        host=platform.platform(),
        started_at=datetime.now(UTC).isoformat(),
        finished=False,
    )
    samples, finished, start = [], False, None
    with options.output.open("x", encoding="utf-8") as output:

        def record(value):
            output.write(json.dumps(value, allow_nan=False) + "\n")
            output.flush()

        def call(index):
            m = pick(mix, seed, index)
            sample = request_sample(m["url"], m["expected"], m["kind"], timeout, m["body"])
            return dict(case=m["case"], **sample)

        def collect(sample):
            samples.append(sample)
            record(dict(type="sample", **sample))

        record(dict(type="metadata", **metadata))
        try:
            for m in mix:
                for _ in range(plan.get("warmup", 10)):
                    sample = request_sample(m["url"], m["expected"], m["kind"], timeout, m["body"])
                    record(dict(type="warmup", case=m["case"], **sample))
                    if sample["outcome"] != "valid":
                        return 1
            start = perf_counter()
            drive(call, plan["concurrency"], 0, plan["duration"], collect)
            finished = True
        finally:
            elapsed = perf_counter() - start if start is not None else 0
            summary = summarize(samples, elapsed)
            summary["by_case"] = {
                m["case"]: summarize([s for s in samples if s["case"] == m["case"]], elapsed)
                for m in mix
            }
            try:
                summary["resources"] = {name: cgroup(path) for name, path in groups.items()}
            except OSError:
                summary["resources"] = None
            record(dict(type="summary", finished=finished, samples=len(samples), **summary))
    return 0 if finished and summary["counts"]["valid"] == len(samples) else 1


if __name__ == "__main__":
    sys.exit(main())
