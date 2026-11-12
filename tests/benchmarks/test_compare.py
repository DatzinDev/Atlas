"""Exercise the standalone probe through real local HTTP, without PostGIS."""

import hashlib
import json
import tempfile
import unittest
from argparse import Namespace
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Thread
from unittest.mock import patch

from benchmarks.compare import check_case, main, mismatch, normalize, request_sample, summarize


def varint(n):
    out = b""
    while n > 0x7F:
        out, n = out + bytes([n & 0x7F | 0x80]), n >> 7
    return out + bytes([n])


def field(number, payload):
    if isinstance(payload, int):
        return varint(number << 3) + varint(payload)
    return varint(number << 3 | 2) + varint(len(payload)) + payload


def packed(*values):
    return b"".join(map(varint, values))


def tile(features, keys=("name", "n"), values=(b"\x0a\x01a", b"\x20\x07")):
    """features: (id, geometry, tags) referencing keys/values by index."""
    body = field(1, b"demo") + field(5, 4096)
    for fid, geometry, tags in features:
        body += field(2, field(1, fid) + field(2, packed(*tags)) + field(3, 1) + field(4, geometry))
    body += b"".join(field(3, k.encode()) for k in keys) + b"".join(field(4, v) for v in values)
    return field(3, body)


POINTS = [(1, packed(9, 2, 4), (0, 0, 1, 1)), (2, packed(9, 6, 8), (1, 1))]


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

    def test_mvt_equivalence_names_count_id_and_quantization_differences(self):
        expected = normalize(tile(POINTS), "mvt")
        # Encoder order of features, keys and values is irrelevant.
        swapped = [(2, packed(9, 6, 8), (0, 1)), (1, packed(9, 2, 4), (0, 1, 1, 0))]
        reordered = tile(swapped, keys=("n", "name"), values=(b"\x0a\x01a", b"\x20\x07"))
        self.assertEqual(normalize(reordered, "mvt"), expected)
        self.assertEqual(expected[0][2][0][3], (("n", ("int", 7)), ("name", ("string", "a"))))
        for features, reason in (
            (POINTS[:1], "count"),
            ([POINTS[0], (3, *POINTS[1][1:])], "ids"),
            ([POINTS[0], (2, packed(9, 6, 10), (1, 1))], "geometry_or_properties"),
        ):
            actual = normalize(tile(features), "mvt")
            self.assertNotEqual(actual, expected)
            self.assertEqual(mismatch(expected, actual, "mvt"), reason)
        geojson = normalize(b'{"features": [{"id": 1}, {"id": 2}]}', "json")
        other = normalize(b'{"features": [{"id": 1}, {"id": 3}]}', "json")
        self.assertEqual(mismatch(geojson, other, "json"), "ids")
        odd, utf8 = tile([(1, packed(9, 2, 4), (0,))]), field(3, field(1, b"\xff"))
        for broken in (tile(POINTS)[:-1], odd, utf8, b"\x1a\x02\x08\x05"):  # varint name
            self.assertRaises(ValueError, normalize, broken, "mvt")

    def test_rust_case_mapping(self):
        def problem(case, path, kind="json", body=None, engine="rust"):
            url = "http://127.0.0.1:8081" + path
            return check_case(Namespace(engine=engine, case=case, url=url, kind=kind, body=body))

        self.assertIsNone(problem("geojson-page", "/api/v1/datasets/demo/items?limit=10"))
        self.assertIsNone(problem("geojson-query", "/api/v1/datasets/demo/query", body=Path("q")))
        self.assertIsNone(problem("mvt-tile", "/prefix/api/v1/datasets/d/tiles/3/1/2.mvt", "mvt"))
        self.assertIsNone(problem("anything", "/", engine="python"))
        self.assertIn("one of", problem("wms", "/api/v1/datasets"))
        self.assertIn("route", problem("export", "/api/v1/datasets/demo/items"))
        self.assertIn("route", problem("catalog", "/api/v1/datasets/../admin/datasets"))
        self.assertIn("--kind mvt", problem("mvt-tile", "/api/v1/datasets/d/tiles/3/1/2.mvt"))
        self.assertIn("POST", problem("geojson-query", "/api/v1/datasets/demo/query"))

    def test_error_codes_resources_and_cancellation(self):
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(413)
                self.end_headers()
                self.wfile.write(b'{"code": "query_too_large", "message": "x"}')

            def log_message(self, *args):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = Thread(target=server.serve_forever)
        thread.start()
        try:
            url = f"http://127.0.0.1:{server.server_port}/api/v1/datasets/demo/items"
            sample = request_sample(url, {}, "json", 2)
            self.assertEqual((sample["outcome"], sample["code"]), ("rejected", "query_too_large"))
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                for name in ("server", "db"):
                    (root / name).mkdir()
                    (root / name / "cpu.stat").write_text("usage_usec 1500000\nuser_usec 1\n")
                    (root / name / "memory.current").write_text("4096\n")
                (root / "expected.json").write_text("{}")
                (root / "atlas").write_bytes(b"release binary")
                args = ["--url", url, "--engine", "rust", "--revision", "r", "--dataset", "d"]
                args += ["--case", "geojson-page", "--expected", str(root / "expected.json")]
                args += ["--output", str(root / "run"), "--pin", str(root / "atlas")]
                args += ["--server-cgroup", str(root / "server"), "--db-cgroup", str(root / "db")]
                self.assertEqual(main([*args, "--warmup", "0", "--samples", "2"]), 1)
                rows = [json.loads(line) for line in (root / "run").read_text().splitlines()]
                self.assertEqual(rows[-1]["counts"]["rejected"], 2)
                digest = hashlib.sha256(b"release binary").hexdigest()
                self.assertEqual(rows[0]["pins"], {str(root / "atlas"): digest})
                usage = {"cpu_seconds": 1.5, "memory_bytes": 4096, "memory_peak_bytes": None}
                self.assertEqual(rows[0]["resources_before"], {"server": usage, "db": usage})
                self.assertEqual(rows[-1]["resources"]["db"], usage)
                (root / "run").unlink()
                with (  # interrupted, and a cgroup vanishes before the final snapshot
                    patch("benchmarks.compare.cgroup", side_effect=[usage, usage, OSError]),
                    patch("benchmarks.compare.request_sample", side_effect=KeyboardInterrupt),
                    self.assertRaises(KeyboardInterrupt),
                ):
                    main(args)
                last = json.loads((root / "run").read_text().splitlines()[-1])
                self.assertFalse(last["finished"])
                self.assertIsNone(last["resources"])
        finally:
            server.shutdown()
            thread.join()
            server.server_close()
