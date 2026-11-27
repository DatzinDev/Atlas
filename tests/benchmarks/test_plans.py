import json
import unittest
from pathlib import Path

from benchmarks.matrix import plan_runs

PLANS = Path(__file__).parents[2] / "benchmarks" / "plans"
CASES = {"rust": 6, "geoserver": 4}


def load(name):
    return json.loads((PLANS / f"{name}.json").read_text())


class PlansTest(unittest.TestCase):
    def test_matrix_plans_follow_perf05(self):
        for engine, cases in CASES.items():
            plan = load(f"matrix-{engine}")
            self.assertEqual(plan["concurrency"], [1, 4, 16])
            self.assertEqual(plan["repetitions"], 3)
            self.assertEqual(plan["options"]["duration"], 60)
            self.assertEqual(plan["options"]["warmup"], 10)
            self.assertEqual(len(dict(plan_runs(plan, Path("out")))), cases * 9)

    def test_soak_plans_follow_perf05(self):
        for engine in CASES:
            plan = load(f"soak-{engine}")
            self.assertEqual((plan["duration"], plan["concurrency"]), (1800, 16))
            weights = {m["case"]: m["weight"] for m in plan["mix"]}
            self.assertEqual(weights, {"geojson-page": 40, "mvt-tile": 40, "geojson-query": 20})

    def test_engine_kinds(self):
        self.assertTrue(
            all(c["options"]["kind"].startswith("gs-") for c in load("matrix-geoserver")["cases"])
        )
        self.assertFalse(
            any(c["options"]["kind"].startswith("gs-") for c in load("matrix-rust")["cases"])
        )
