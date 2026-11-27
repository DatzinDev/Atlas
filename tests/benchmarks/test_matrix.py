import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from benchmarks.matrix import main, plan_runs

PLAN = dict(
    engine="rust",
    revision="abc",
    dataset="d",
    concurrency=[1, 4],
    repetitions=3,
    options={"samples": 1000, "pin": ["a", "b"]},
    cases=[dict(case="catalog", url="http://x", expected="e.json", options={"kind": "json"})],
)


class MatrixTest(unittest.TestCase):
    def test_plan_expands_to_one_run_per_combination(self):
        runs = dict(plan_runs(PLAN, Path("out")))
        self.assertEqual(len(runs), 6)
        run = runs["rust-catalog-c4-r2"]
        self.assertEqual(run[run.index("--concurrency") + 1], "4")
        self.assertEqual(run.count("--pin"), 2)
        self.assertIn("--kind", run)

    def test_finished_runs_are_skipped_and_partial_ones_stop(self):
        with tempfile.TemporaryDirectory() as tmp:
            plan = Path(tmp, "plan.json")
            plan.write_text(json.dumps({**PLAN, "concurrency": [1], "repetitions": 1}))
            args = ["--plan", str(plan), "--output-dir", tmp]
            out = Path(tmp, "rust-catalog-c1-r1.jsonl")
            with patch("benchmarks.matrix.compare", return_value=0) as run:
                self.assertEqual(main(args), 0)
                run.assert_called_once()
            out.write_text(json.dumps(dict(type="summary", finished=True)) + "\n")
            with patch("benchmarks.matrix.compare") as run:
                self.assertEqual(main(args), 0)
                run.assert_not_called()
            out.write_text(json.dumps(dict(type="sample")) + "\n")
            with self.assertRaises(FileExistsError):
                main(args)

    def test_failed_run_sets_exit_code(self):
        with tempfile.TemporaryDirectory() as tmp:
            plan = Path(tmp, "plan.json")
            plan.write_text(json.dumps({**PLAN, "concurrency": [1], "repetitions": 1}))
            with patch("benchmarks.matrix.compare", return_value=1):
                self.assertEqual(main(["--plan", str(plan), "--output-dir", tmp]), 1)
