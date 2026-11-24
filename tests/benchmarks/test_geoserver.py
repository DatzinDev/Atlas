import json
import os
import re
from argparse import Namespace
from urllib.parse import parse_qs, urlsplit
from urllib.request import urlopen

import pytest
from benchmarks.geoserver import cases, equivalence, setup
from psycopg.conninfo import conninfo_to_dict

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


BASE = "http://127.0.0.1:8081/geoserver"
COLUMNS = ("geom", "id")


def params(url):
    return {k: v[0] for k, v in parse_qs(urlsplit(url).query, keep_blank_values=True).items()}


def page(query, case="geojson-query"):
    return params(cases.wfs(BASE, "points", query, case, COLUMNS, 1000))


def test_filter_ast_becomes_ecql_with_quoted_fields_and_literals():
    node = {
        "op": "and",
        "args": [
            {"op": "gte", "field": "poblacion", "value": 100000},
            {"op": "not", "arg": {"op": "in", "field": "nombre", "value": ["a", "o'k"]}},
            {
                "op": "or",
                "args": [
                    {"op": "is_null", "field": "x"},
                    {"op": "eq", "field": "b", "value": True},
                ],
            },
            {"op": "ilike", "field": "nombre", "value": "san%"},
        ],
    }
    assert cases.cql(node, "geom") == (
        "(\"poblacion\" >= 100000 AND (NOT \"nombre\" IN ('a', 'o''k')) AND "
        '("x" IS NULL OR "b" = TRUE) AND "nombre" ILIKE \'san%\')'
    )


def test_spatial_filters_keep_wgs84_and_every_polygon():
    ring = [[0, 0], [1, 0], [1, 1], [0, 0]]
    within = {"op": "within", "geometry": {"type": "MultiPolygon", "coordinates": [[ring], [ring]]}}
    text = cases.cql(within, "geom")
    assert text.startswith("WITHIN(geom, SRID=4326;MULTIPOLYGON(((0 0, 1 0, 1 1, 0 0)), ((")
    for bad in ({"op": "between"}, {"op": "eq", "field": 'a"b', "value": 1}):
        with pytest.raises(ValueError):
            cases.cql(bad, "geom")


def test_page_maps_limit_offset_sort_bbox_and_properties():
    query = {"bbox": [-104, 20, -102, 22], "properties": ["nombre"], "sort": ["nombre:desc"]}
    p = page({**query, "limit": 10, "offset": 20, "filter": {"op": "is_not_null", "field": "n"}})
    assert p["typeNames"] == "atlas_bench:points"
    assert p["srsName"] == "EPSG:4326" and p["outputFormat"] == "application/json"
    assert p["count"] == "10" and p["startIndex"] == "20"
    assert p["sortBy"] == "nombre D,id A"
    assert p["propertyName"] == "nombre,geom"
    assert p["CQL_FILTER"] == "BBOX(geom, -104, 20, -102, 22, 'EPSG:4326') AND \"n\" IS NOT NULL"
    assert page({})["count"] == "1000" and page({"sort": ["id:desc"]})["sortBy"] == "id D"


def test_export_is_complete_and_rejects_paging():
    p = page({"properties": ["nombre"]}, case="export")
    assert "count" not in p and "startIndex" not in p and p["sortBy"] == "id A"
    for bad in ({"limit": 1}, {"offset": 1}, {"sort": ["a"]}, {"cursor": 1}):
        with pytest.raises(ValueError):
            page(bad, case="export")
    with pytest.raises(ValueError, match="sort"):
        page({"sort": ["a:up"]})


def test_tile_requests_extent_4096_and_buffer_64_over_the_xyz_envelope():
    p = params(cases.tile(BASE, "points", 1, 1, 0))
    assert [float(v) for v in p["bbox"].split(",")] == [0, 0, cases.ORIGIN, cases.ORIGIN]
    assert p["width"] == p["height"] == "256" and p["buffer"] == "4"
    assert p["format"] == "application/vnd.mapbox-vector-tile" and p["crs"] == "EPSG:3857"
    with pytest.raises(ValueError):
        cases.tile(BASE, "points", 1, 2, 0)


def test_cli_reports_not_applicable_cases_instead_of_failing(capsys):
    for case in ("catalog", "tilejson"):
        assert cases.main(["--case", case, "--layer", "points"]) == 0
        out = json.loads(capsys.readouterr().out)
        assert out["status"] == "n/a" and out["engine"] == "geoserver" and out["reason"]
    assert cases.main(["--case", "mvt-tile", "--layer", "points", "--tile", "0/0/0"]) == 0
    assert json.loads(capsys.readouterr().out)["kind"] == "mvt"
    with pytest.raises(SystemExit):
        cases.main(["--case", "mvt-tile", "--layer", "points"])


def test_geojson_drops_the_geoserver_envelope_and_prefixed_ids():
    body = json.dumps(
        {
            "type": "FeatureCollection",
            "timeStamp": "2026-11-24T00:00:00Z",
            "totalFeatures": 1,
            "features": [
                {"id": "eq.7", "geometry": None, "properties": {"id": 7, "label": "f0"}},
                {"id": "eq.x", "geometry": None, "properties": {}},
            ],
        }
    )
    assert equivalence.geojson(body) == [(7, None, {"label": "f0"}), ("eq.x", None, {})]


def test_difference_tolerates_one_quantum_of_nine_decimals():
    point = {"type": "Point", "coordinates": [10.0, 20.0]}
    one = {"type": "Point", "coordinates": [10.000000001, 20.0]}
    two = {"type": "Point", "coordinates": [10.000000002, 20.0]}
    assert abs(10.000000001 - 10.0) > 1e-9
    assert equivalence.difference([(1, point, {})], [(1, one, {})]) is None
    assert equivalence.difference([(1, point, {})], [(1, two, {})]) == "geometry"
    assert equivalence.difference([(1, point, {})], [(2, point, {})]) == "ids"
    assert equivalence.difference([(1, point, {"a": 1})], [(1, point, {})]) == "properties"
    assert equivalence.difference([(1, point, {})], []) == "count"


GEOSERVER = os.environ.get("ATLAS_GEOSERVER_URL")
FIXTURE = """
CREATE SCHEMA run_eq;
CREATE TABLE run_eq.features (id bigint PRIMARY KEY, label text, value float8,
                              geom geometry(Geometry, 3857));
INSERT INTO run_eq.features
SELECT i, 'f' || (i % 7), i / 3.0,
       CASE WHEN i % 2 = 0 THEN ST_SetSRID(ST_Point(i * 1700.5, i * 900.25), 3857)
            ELSE ST_MakeEnvelope(i * 1700, i * 900, i * 1700 + 1500, i * 900 + 700, 3857) END
FROM generate_series(1, 120) i;
CREATE INDEX ON run_eq.features USING GIST (geom);
ANALYZE run_eq.features;
"""
PAGE = """
SELECT id, ST_AsGeoJSON(ST_Transform(geom, 4326))::json AS geometry,
       json_build_object('label', label, 'value', value) AS properties FROM run_eq.features
WHERE ST_Intersects(geom, ST_Transform(ST_MakeEnvelope(0.1, 0.05, 1.5, 0.8, 4326), 3857))
ORDER BY label DESC, id LIMIT 7 OFFSET 3
"""
TILE = """
SELECT ST_AsMVT(q, 'eq', 4096, 'geom', 'id') AS tile FROM (
    SELECT id, label, value, ST_AsMVTGeom(geom, ST_TileEnvelope(8, 128, 127), 4096, 64) AS geom
    FROM run_eq.features WHERE geom && ST_TileEnvelope(8, 128, 127, margin => 64 / 4096.0)
) q
"""


@pytest.mark.skipif(not GEOSERVER, reason="ATLAS_GEOSERVER_URL is not set")
async def test_geoserver_bodies_match_postgis_oracles(bench_db, database_url):
    async with bench_db.connection() as conn:
        await conn.execute(FIXTURE)
        name = (await (await conn.execute("SELECT current_database() AS name")).fetchone())["name"]
        rows = await (await conn.execute(PAGE)).fetchall()
        oracle_tile = (await (await conn.execute(TILE)).fetchone())["tile"]
    info = conninfo_to_dict(database_url)
    rest = setup.Rest(GEOSERVER, "admin", os.environ["GEOSERVER_ADMIN_PASSWORD"])
    args = Namespace(
        db_host=os.environ.get("ATLAS_GEOSERVER_DB_HOST", "postgis"),
        db_port=5432,
        database=name,
        schema="run_eq",
        db_user=info["user"],
    )
    setup.configure(rest, setup.store_parameters(args), info["password"], [("eq", "features")])
    try:
        query = {
            "bbox": [0.1, 0.05, 1.5, 0.8],
            "properties": ["label", "value"],
            "sort": ["label:desc"],
            "limit": 7,
            "offset": 3,
        }
        url = cases.wfs(GEOSERVER, "eq", query, "geojson-query", ("geom", "id"), 1000)
        with urlopen(url) as response:
            body = response.read()
        assert "numberMatched" not in json.loads(body)
        actual = equivalence.geojson(body)
        assert len(actual) == 7
        assert (
            equivalence.difference(
                [(r["id"], r["geometry"], r["properties"]) for r in rows], actual
            )
            is None
        )
        with urlopen(cases.tile(GEOSERVER, "eq", 8, 128, 127)) as response:
            report = equivalence.coverage(bytes(oracle_tile), response.read())
        assert report["eq"]["extent"] == [4096, 4096]
        assert report["eq"]["features"][0] > 0
        assert report["eq"]["missing"] == [] and report["eq"]["extra"] == []
    finally:
        rest.call("DELETE", f"/rest/workspaces/{setup.WORKSPACE}?recurse=true", ok=(200, 404))
