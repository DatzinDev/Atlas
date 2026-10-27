"""Exercise the standalone probe through real local HTTP, without PostGIS."""

import json
import tempfile
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Thread
from unittest.mock import patch

from benchmarks.compare import main, normalize, request_sample, summarize


class ProbeTests(unittest.TestCase):
    def test_http_integrity_rejections_errors_and_cli(self):
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                code = int(self.path[1:]) if self.path[1:].isdigit() else 200
                self.send_response(code)
                self.end_headers()
                self.wfile.write(b"{" if self.path == "/malformed" else b'{"features": []}')

            def log_message(self, *args):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = Thread(target=server.serve_forever)
        thread.start()
        url = f"http://127.0.0.1:{server.server_port}"
        try:
            expected = normalize(b'{"features": []}', "json")
            for path, outcome in (
                ("ok", "valid"),
                ("503", "rejected"),
                ("500", "error"),
                ("302", "error"),
                ("malformed", "invalid"),
            ):
                self.assertEqual(
                    request_sample(f"{url}/{path}", expected, "json", 2)["outcome"], outcome
                )
            self.assertEqual(request_sample(url + "/ok", {}, "json", 2)["outcome"], "invalid")
            with tempfile.TemporaryDirectory() as directory:
                expected_file, output = Path(directory) / "expected.json", Path(directory) / "run"
                expected_file.write_text('{"features": []}')
                args = [
                    "--url",
                    url + "/ok",
                    "--engine",
                    "fixture",
                    "--revision",
                    "test",
                    "--dataset",
                    "empty",
                    "--case",
                    "page",
                    "--expected",
                    str(expected_file),
                    "--output",
                    str(output),
                    "--samples",
                    "7",
                    "--concurrency",
                    "3",
                ]
                self.assertEqual(main(args), 0)
                rows = [json.loads(line) for line in output.read_text().splitlines()]
                self.assertEqual(rows[-1]["counts"]["valid"], 7)
                self.assertTrue(rows[-1]["finished"])
                with self.assertRaises(FileExistsError):
                    main(args)
                output.unlink()
                with (
                    patch("benchmarks.compare.request_sample", side_effect=KeyboardInterrupt),
                    self.assertRaises(KeyboardInterrupt),
                ):
                    main(args)
                self.assertFalse(json.loads(output.read_text().splitlines()[-1])["finished"])
        finally:
            server.shutdown()
            thread.join()
            server.server_close()

    def test_summary_and_numeric_integrity(self):
        result = summarize(
            [dict(outcome="valid", ms=10, bytes=4), dict(outcome="invalid", ms=999, bytes=99)], 2
        )
        self.assertEqual(result["p95_ms"], 10)
        self.assertEqual(result["valid_requests_per_second"], 0.5)
        self.assertNotEqual(normalize(b"1", "json"), normalize(b"true", "json"))
        self.assertNotEqual(normalize(b"1.00000000000000001", "json"), normalize(b"1.0", "json"))
        with patch("benchmarks.compare.build_opener") as opener:
            opener.return_value.open.side_effect = TimeoutError
            self.assertEqual(
                request_sample("http://localhost", b"", "bytes", 1)["outcome"], "error"
            )
