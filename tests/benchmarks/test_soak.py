import json
import tempfile
import unittest
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Thread

from benchmarks.soak import main, pick

MIX = [dict(case="page", weight=4), dict(case="tile", weight=4), dict(case="filter", weight=2)]


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'{"features": []}')

    def log_message(self, *args):
        pass


class SoakTest(unittest.TestCase):
    def test_pick_is_deterministic_and_follows_the_weights(self):
        first = [pick(MIX, 7, i)["case"] for i in range(5000)]
        self.assertEqual(first, [pick(MIX, 7, i)["case"] for i in range(5000)])
        share = {k: v / 5000 for k, v in Counter(first).items()}
        for case, expected in (("page", 0.4), ("tile", 0.4), ("filter", 0.2)):
            self.assertAlmostEqual(share[case], expected, delta=0.03)

    def test_run_writes_mix_samples_and_per_case_summary(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = Thread(target=server.serve_forever)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as tmp:
                expected = Path(tmp, "expected.json")
                expected.write_text('{"features": []}')
                url = f"http://127.0.0.1:{server.server_port}/"
                mix = [dict(m, url=url, expected=str(expected)) for m in MIX]
                plan = Path(tmp, "plan.json")
                plan.write_text(
                    json.dumps(
                        dict(engine="fixture", concurrency=2, duration=0.3, warmup=1, mix=mix)
                    )
                )
                output = Path(tmp, "soak.jsonl")
                self.assertEqual(main(["--plan", str(plan), "--output", str(output)]), 0)
                rows = [json.loads(line) for line in output.read_text().splitlines()]
                summary = rows[-1]
                self.assertTrue(summary["finished"])
                self.assertGreater(summary["samples"], 3)
                self.assertEqual(set(summary["by_case"]), {"page", "tile", "filter"})
                self.assertEqual(summary["counts"]["valid"], summary["samples"])
        finally:
            server.shutdown()
            server.server_close()
            thread.join()
