"""Full filtered downloads, projection, access control and cursor cleanup on disconnect."""

import csv
import json
from io import StringIO
from unittest.mock import AsyncMock, Mock, patch
from urllib.parse import urlencode

import pytest
from psycopg import sql
from starlette.requests import ClientDisconnect, Request

from atlas.api.download import build_router
from atlas.exports.base import available_formats


async def test_full_filtered_download_and_projection(client, atlas):
    atlas.context.config = atlas.context.config.model_copy(update={"default_limit": 1})
    response = await client.get("/atlas/layers/municipios/download")
    assert response.status_code == 200 and len(response.json()["features"]) == 5
    assert response.headers["content-disposition"] == 'attachment; filename="municipios.geojson"'
    response = await client.get(
        "/atlas/layers/municipios/download",
        params={
            "poblacion__gt": "100000",
            "properties": "nombre,clave",
            "bbox": "-103.1,20.4,-101.4,21.6",
        },
    )
    expected = await atlas.context.db.fetch_one(
        sql.SQL("SELECT count(*) AS n FROM geo.municipios WHERE poblacion > %s"), [100000]
    )
    assert len(response.json()["features"]) == expected["n"] == 3
    assert set(response.json()["features"][0]["properties"]) == {"nombre", "clave", "id"}
    schools = (await client.get("/atlas/layers/escuelas/download")).json()
    assert schools["features"][0]["geometry"]["coordinates"] == pytest.approx(
        [-102.8, 21.3], abs=0.01
    )
    assert schools["crs"]["properties"]["name"] == "EPSG:4326"


@pytest.mark.parametrize(
    "mode,extra", [("wkt", ["geometry"]), ("centroid", ["lon", "lat"]), ("none", [])]
)
async def test_csv_modes_and_projected_headers(client, mode, extra):
    response = await client.get(
        "/atlas/layers/escuelas/download",
        params={"format": "csv", "properties": "nombre", "geometry": mode},
    )
    assert response.status_code == 200 and response.content.startswith(b"\xef\xbb\xbf")
    rows = list(csv.reader(StringIO(response.content.decode("utf-8-sig"))))
    assert rows[0] == ["id", "nombre", *extra] and len(rows) == 6
    if mode == "centroid":
        assert float(rows[1][-2]) == pytest.approx(-102.8, abs=0.01)


async def test_permissions_formats_and_download_limit(client, atlas):
    atlas.context.permission_checker = lambda layer, op, user: op == "download"
    assert (await client.get("/atlas/layers/municipios/download")).status_code == 200
    atlas.context.permission_checker = lambda layer, op, user: op == "view"
    assert (await client.get("/atlas/layers/municipios/download")).status_code == 403
    atlas.context.permission_checker = None
    atlas.context.registry.get("municipios").downloadable = False
    assert (await client.get("/atlas/layers/municipios/download")).status_code == 403
    atlas.context.registry.get("municipios").downloadable = True
    unsupported = await client.get("/atlas/layers/municipios/download?format=xml")
    assert (
        unsupported.status_code == 400
        and unsupported.json()["error"]["details"]["formats"] == available_formats()
    )
    atlas.context.config = atlas.context.config.model_copy(update={"max_download_features": 2})
    assert (await client.get("/atlas/layers/municipios/download")).status_code == 413
    assert (
        len((await client.get("/atlas/layers/municipios/download?clave=001")).json()["features"])
        == 1
    )


@pytest.mark.parametrize(
    "query",
    [
        "properties=pg_sleep(10)",
        "nombre'; DROP TABLE--=x",
        "poblacion__union=1",
        "format=csv&geometry=bad",
        "bbox=nan,0,1,1",
        "format=csv&format=geojson",
    ],
)
async def test_hostile_and_invalid_input(client, query):
    response = await client.get("/atlas/layers/municipios/download?" + query)
    assert response.status_code == 400


async def test_cursor_closed_on_early_exit_and_before_headers(atlas, monkeypatch):
    consumed = []
    closed = []
    original = atlas.context.db.stream

    async def observed(*args, **kwargs):
        source = original(*args, **kwargs)
        try:
            async for row in source:
                consumed.append(row)
                yield row
        finally:
            await source.aclose()
            closed.append(True)

    monkeypatch.setattr(atlas.context.db, "stream", observed)
    route = next(
        r for r in build_router(atlas.context).routes if r.path == "/layers/{name}/download"
    )
    request = Request(
        {
            "type": "http",
            "query_string": urlencode(
                {"ATLAS_FILTER": '{"op":"gte","field":"id","value":1}'}
            ).encode(),
            "headers": [],
        }
    )
    response = await route.endpoint("municipios", request, None)
    assert len(consumed) == 1
    await anext(response.body_iterator)
    await response.body_iterator.aclose()
    assert closed == [True]
    response = await route.endpoint("municipios", request, None)

    async def send(message):
        raise OSError("client disconnected")

    async def receive():
        return {"type": "http.disconnect"}

    with pytest.raises((ClientDisconnect, ExceptionGroup)):
        await response({"type": "http", "asgi": {"spec_version": "2.4"}}, receive, send)
    assert closed == [True, True]


async def test_geopackage_download_and_header_disconnect_cleanup(
    client, atlas, monkeypatch, tmp_path
):
    fiona = pytest.importorskip("fiona")
    from tempfile import TemporaryDirectory

    scratch = tmp_path / "scratch"
    scratch.mkdir()
    monkeypatch.setattr(
        "atlas.exports.geopackage.TemporaryDirectory",
        lambda **kwargs: TemporaryDirectory(dir=scratch, **kwargs),
    )
    response = await client.get("/atlas/layers/escuelas/download?format=gpkg")
    assert response.status_code == 200
    assert response.headers["content-type"] == "application/geopackage+sqlite3"
    path = tmp_path / "schools.gpkg"
    path.write_bytes(response.content)
    with fiona.open(path, layer="escuelas") as source:
        assert len(source) == 5 and source.crs.to_epsg() == 4326
    assert list(scratch.iterdir()) == []
    route = next(
        r for r in build_router(atlas.context).routes if r.path == "/layers/{name}/download"
    )
    request = Request(
        {
            "type": "http",
            "query_string": urlencode(
                {"format": "gpkg", "ATLAS_FILTER": '{"op":"gte","field":"id","value":1}'}
            ).encode(),
            "headers": [],
        }
    )
    response = await route.endpoint("escuelas", request, None)
    assert list(scratch.iterdir())

    async def send(message):
        raise OSError("disconnected before headers")

    async def receive():
        return {"type": "http.disconnect"}

    with pytest.raises((ClientDisconnect, ExceptionGroup)):
        await response({"type": "http", "asgi": {"spec_version": "2.4"}}, receive, send)
    assert list(scratch.iterdir()) == []


async def test_shapefile_download_warning_limits_and_cleanup(client, atlas, monkeypatch, tmp_path):
    pytest.importorskip("fiona")
    from io import BytesIO
    from tempfile import TemporaryDirectory
    from zipfile import ZipFile

    monkeypatch.setattr(
        "atlas.exports.shapefile.TemporaryDirectory",
        lambda **kwargs: TemporaryDirectory(dir=tmp_path, **kwargs),
    )
    response = await client.get("/atlas/layers/municipios/download?format=shp")
    assert response.status_code == 200 and response.headers["content-type"] == "application/zip"
    assert response.headers["content-disposition"].endswith('municipios.zip"')
    assert "README.txt" in response.headers["warning"]
    with ZipFile(BytesIO(response.content)) as archive:
        assert "municipios.shp" in archive.namelist() and "README.txt" in archive.namelist()
    assert list(tmp_path.iterdir()) == []
    route = next(
        r for r in build_router(atlas.context).routes if r.path == "/layers/{name}/download"
    )
    request = Request(
        {
            "type": "http",
            "query_string": urlencode(
                {"format": "shp", "ATLAS_FILTER": '{"op":"gte","field":"id","value":1}'}
            ).encode(),
            "headers": [],
        }
    )
    response = await route.endpoint("municipios", request, None)
    await anext(response.body_iterator)
    assert list(tmp_path.iterdir())
    await response.body_iterator.aclose()
    assert list(tmp_path.iterdir()) == []
    monkeypatch.setattr("atlas.exports.shapefile.MAX_BYTES", 10)
    response = await client.get("/atlas/layers/municipios/download?format=shp")
    assert response.status_code == 413 and response.json()["error"]["code"] == "query_too_large"
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize("format", ["geojson", "csv"])
async def test_structured_download_is_complete_and_matches_cap_count(client, atlas, format):
    atlas.context.config = atlas.context.config.model_copy(
        update={"default_limit": 1, "max_limit": 1, "max_download_features": 2}
    )
    params = {
        "format": format,
        "ATLAS_FILTER": json.dumps({"op": "in", "field": "id", "value": [1, 2, 4]}),
        "id__gte": "2",
        "bbox": "-104,19,-100,23",
        "properties": "nombre",
        "count": "1",
    }
    db = atlas.context.db
    with (
        patch.object(db, "fetch_one", wraps=db.fetch_one) as count,
        patch.object(db, "stream", wraps=db.stream) as stream,
    ):
        response = await client.get("/atlas/layers/escuelas/download", params=params)
    assert response.status_code == 200, response.text
    assert count.call_args.args[1] == stream.call_args.args[1][1:-2]
    if format == "geojson":
        features = response.json()["features"]
        ids = [f["id"] for f in features]
        assert set(features[0]["properties"]) == {"id", "nombre"}
    else:
        ids = [int(r["id"]) for r in csv.DictReader(StringIO(response.content.decode("utf-8-sig")))]
    assert ids == [2, 4]


@pytest.mark.parametrize(
    "params",
    [
        {"atlas_filter": "{"},
        {"atlas_filter": '{"op":"eq","field":"missing","value":1}'},
        {"atlas_filter": '{"op":"eq","field":"id","value":"1"}'},
        {"ATLAS_FILTER": "null", "atlas_filter": "null"},
        {"CQL_FILTER": ""},
        {"FILTER": ""},
        {"ATLAS_FILTERS": "null"},
    ],
)
async def test_invalid_structured_download_never_queries(client, atlas, monkeypatch, params):
    monkeypatch.setattr(
        atlas.context.db, "fetch_one", AsyncMock(side_effect=AssertionError("unexpected SQL"))
    )
    monkeypatch.setattr(
        atlas.context.db, "stream", Mock(side_effect=AssertionError("unexpected cursor"))
    )
    response = await client.get("/atlas/layers/escuelas/download", params=params)
    assert response.status_code == 400


async def test_filtered_download_cap_and_denial_do_not_start_export(client, atlas, monkeypatch):
    params = {"format": "gpkg", "atlas_filter": '{"op":"lte","field":"id","value":2}'}
    atlas.context.config = atlas.context.config.model_copy(update={"max_download_features": 1})
    monkeypatch.setattr(
        atlas.context.db, "stream", Mock(side_effect=AssertionError("unexpected cursor"))
    )
    monkeypatch.setattr(
        "atlas.exports.geopackage.TemporaryDirectory",
        Mock(side_effect=AssertionError("unexpected temporary file")),
    )
    response = await client.get("/atlas/layers/escuelas/download", params=params)
    assert response.status_code == 413
    atlas.context.permission_checker = lambda *args: False
    monkeypatch.setattr(
        atlas.context.db, "fetch_one", AsyncMock(side_effect=AssertionError("unexpected SQL"))
    )
    assert (await client.get("/atlas/layers/escuelas/download", params=params)).status_code == 403
