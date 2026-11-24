import json
import re
from argparse import Namespace

import pytest
from benchmarks.geoserver import setup

ARGS = Namespace(
    db_host="postgis", db_port=5432, database="atlas_bench_x", schema="run_x", db_user="atlas"
)


class FakeRest:
    def __init__(self):
        self.calls = []

    def call(self, method, path, body=None, ok=(200, 201)):
        self.calls.append((method, path, body))


def test_refuses_databases_outside_the_benchmark_prefix(tmp_path, monkeypatch):
    monkeypatch.setenv("GEOSERVER_ADMIN_PASSWORD", "admin-secret")
    monkeypatch.setenv("ATLAS_BENCH_DB_PASSWORD", "db-secret")
    for name in ("atlas", "postgres", "bench_atlas", "atlas_benchx"):
        argv = ["--database", name, "--schema", "s", "--layer", "a=t"]
        with pytest.raises(ValueError, match="atlas_bench_"):
            setup.main([*argv, "--url", "http://192.0.2.1:9", "--output", str(tmp_path / "m")])
    setup.check_database("atlas_bench_run")


def test_image_and_plugin_are_pinned_to_one_version():
    text = setup.DOCKERFILE.read_text()
    base = re.search(r"^FROM \S+:(\d+\.\d+\.\d+)@sha256:[0-9a-f]{64}$", text, re.M)
    assert base, "base image must carry a version and a digest"
    assert "latest" not in text
    assert re.search(r"ADD --checksum=sha256:[0-9a-f]{64}", text)
    plugins = re.findall(r"geoserver-(\d+\.\d+\.\d+)-vectortiles-plugin\.zip", text)
    assert plugins and set(plugins) == {base.group(1)}


def test_password_reaches_the_store_but_never_the_manifest():
    rest = FakeRest()
    parameters = setup.store_parameters(ARGS)
    setup.configure(rest, parameters, "db-secret", [("points", "features")])
    store = next(body for method, path, body in rest.calls if path.endswith("/datastores"))
    entries = {e["@key"]: e["$"] for e in store["dataStore"]["connectionParameters"]["entry"]}
    assert entries["passwd"] == "db-secret"
    assert entries["max connections"] == "10"
    versions = {"about": {"resource": [{"@name": "GeoServer", "Version": "3.0.2"}]}}
    manifest = setup.manifest(versions, "3.0.2", parameters, [("points", "features")], None)
    text = json.dumps(manifest)
    assert "db-secret" not in text and "passwd" not in text
    assert manifest["versions"] == {"GeoServer": "3.0.2"}
    assert manifest["config"]["datastore"]["database"] == "atlas_bench_x"


def test_setup_only_touches_its_workspace_and_drops_tile_caching():
    rest = FakeRest()
    setup.configure(rest, setup.store_parameters(ARGS), "pw", [("points", "features")])
    mutating = [(m, p) for m, p, _ in rest.calls if m != "GET"]
    assert all(setup.WORKSPACE in p for _, p in mutating if p != "/rest/workspaces")
    created = next(b for m, p, b in rest.calls if p == "/rest/workspaces")
    assert created == {"workspace": {"name": setup.WORKSPACE}}
    assert ("DELETE", "/gwc/rest/layers/atlas_bench:points") in mutating
    feature = next(b for m, p, b in rest.calls if p.endswith("/featuretypes"))["featureType"]
    assert feature["srs"] == "EPSG:3857" and feature["nativeName"] == "features"


def test_missing_container_stops_before_touching_geoserver(tmp_path, monkeypatch):
    monkeypatch.setenv("GEOSERVER_ADMIN_PASSWORD", "admin-secret")
    monkeypatch.setenv("ATLAS_BENCH_DB_PASSWORD", "db-secret")
    monkeypatch.setattr(setup, "Rest", lambda *a: pytest.fail("GeoServer was contacted"))
    monkeypatch.setattr(setup.subprocess, "run", lambda *a, **k: Namespace(returncode=1, stdout=""))
    argv = ["--database", "atlas_bench_x", "--schema", "s", "--layer", "a=t"]
    with pytest.raises(SystemExit, match="--container"):
        setup.main([*argv, "--container", "other-geoserver-1", "--output", str(tmp_path / "m")])
    assert not (tmp_path / "m").exists()


def test_existing_output_is_never_overwritten(tmp_path, monkeypatch):
    out = tmp_path / "manifest.json"
    out.write_text("{}")
    argv = ["--database", "atlas_bench_x", "--schema", "s", "--layer", "a=t", "--output", str(out)]
    with pytest.raises(SystemExit, match="exists"):
        setup.main(argv)
    assert out.read_text() == "{}"
