"""Run the PERF05 matrix for one engine: cases x concurrency x repetitions, one JSONL each."""

import argparse
import json
import sys
from pathlib import Path

from benchmarks.compare import main as compare


def argv(plan, case, concurrency, repetition, directory):
    name = f"{plan['engine']}-{case['case']}-c{concurrency}-r{repetition}"
    options = {**plan.get("options", {}), **case.get("options", {})}
    out = ["--engine", plan["engine"], "--revision", plan["revision"]]
    out += ["--dataset", plan["dataset"], "--case", case["case"], "--url", case["url"]]
    out += ["--expected", case["expected"], "--concurrency", str(concurrency)]
    out += ["--output", str(directory / f"{name}.jsonl")]
    for flag, value in options.items():
        for item in value if isinstance(value, list) else [value]:
            out += [f"--{flag}", str(item)]
    return name, out


def plan_runs(plan, directory):
    for case in plan["cases"]:
        for concurrency in plan["concurrency"]:
            for repetition in range(1, plan["repetitions"] + 1):
                yield argv(plan, case, concurrency, repetition, directory)


def done(path):
    """True only for a run that wrote a finished summary; a partial file blocks the retry."""
    last = json.loads(path.read_text().splitlines()[-1])
    if last.get("type") == "summary" and last.get("finished"):
        return True
    raise FileExistsError(f"{path} is incomplete; move it away to repeat the run")


def main(args=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    options = parser.parse_args(args)
    plan = json.loads(options.plan.read_text())
    options.output_dir.mkdir(parents=True, exist_ok=True)
    failed = 0
    for name, run in plan_runs(plan, options.output_dir):
        target = Path(run[run.index("--output") + 1])
        if target.exists() and done(target):
            status = "skipped"
        else:
            status = "ok" if compare(run) == 0 else "failed"
        failed += status == "failed"
        print(json.dumps({"run": name, "status": status}), flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
