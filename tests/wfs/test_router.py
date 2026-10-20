"""HTTP dispatch, validation, auth, logging and proxy-aware capabilities."""

import json
from unittest.mock import AsyncMock, Mock, patch
from urllib.parse import urlencode
from xml.etree import ElementTree as ET

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient


@pytest.mark.parametrize("key", ["REQUEST", "request", "Request"])
async def test_case_insensitive_parameters(client, key):
    response = await client.get("/atlas/wfs", params={key: "GetCapabilities", "SeRvIcE": "WFS"})
    assert response.status_code == 200
    assert response.headers["content-type"].startswith("application/xml")
    assert ET.fromstring(response.text).attrib["version"] == "2.0.0"


@pytest.mark.parametrize(
    "params,code",
    [
        ({"REQUEST": "unknown"}, "invalid_parameter"),
        ({"REQUEST": "GetCapabilities", "SERVICE": "WMS"}, "invalid_parameter"),
        ({"REQUEST": "GetCapabilities", "VERSION": "1.1.0"}, "invalid_parameter"),
        ({"REQUEST": "GetFeature", "typeNames": "municipios", "count": "zero"}, "invalid_filter"),
        (
            {"REQUEST": "GetFeature", "typeNames": "municipios", "outputFormat": "GML"},
            "unsupported_format",
        ),
        (
            {
                "REQUEST": "DescribeFeatureType",
                "typeNames": "municipios",
                "outputFormat": "text/xml",
            },
            "unsupported_format",
        ),
        (
            {"REQUEST": "GetFeature", "typeNames": "municipios", "poblacion__union": "1"},
            "invalid_filter",
        ),
        (
            {"REQUEST": "GetFeature", "typeNames": "municipios", "bbox": "nan,0,1,1"},
            "invalid_geometry",
        ),
    ],
)
async def test_error_responses(client, params, code):
    response = await client.get("/atlas/wfs", params=params)
    assert response.status_code == 400 and response.json()["error"]["code"] == code


async def test_form_encoded_get_feature_and_schema(client, atlas):
    reports = []
    atlas.context.log_hook = reports.append
    response = await client.post(
        "/atlas/wfs",
        content=urlencode(
            {
                "request": "GetFeature",
                "typeNames": "municipios",
                "count": "2",
                "offset": "1",
                "propertyName": "nombre",
            }
        ),
        headers={"content-type": "application/x-www-form-urlencoded"},
    )
    assert (
        response.status_code == 200 and response.headers["content-type"] == "application/geo+json"
    )
    data = response.json()
    assert data["numberReturned"] == 2 and data["startIndex"] == 1 and data["numberMatched"] == 5
    assert set(data["features"][0]["properties"]) == {"nombre", "id"}
    assert reports[-1].feature_count == 2 and reports[-1].error is None
    schema = await client.get(
        "/atlas/wfs", params={"request": "DescribeFeatureType", "typeNames": "municipios"}
    )
    assert schema.status_code == 200 and schema.headers["content-type"] == "application/json"
    assert schema.json()["geometry_type"] == "Polygon"
    assert len(reports) == 2


async def test_permissions_filter_capabilities_and_deny_queries(client, atlas):
    reports = []
    atlas.context.log_hook = reports.append
    atlas.context.permission_checker = lambda *args: False
    caps = await client.get("/atlas/wfs?request=GetCapabilities")
    assert "<wfs:FeatureType>" not in caps.text
    for operation in ["GetFeature", "DescribeFeatureType"]:
        denied = await client.get(
            "/atlas/wfs", params={"request": operation, "typeNames": "municipios"}
        )
        assert denied.status_code == 403
        assert reports[-1].error == "permission_denied"
    assert (await client.get("/atlas/wfs?request=GetFeature&typeNames=missing")).status_code == 404


async def test_xml_and_duplicate_parameters_are_rejected(client):
    assert (
        await client.post(
            "/atlas/wfs", content="<GetFeature/>", headers={"content-type": "text/xml"}
        )
    ).status_code == 400
    assert (
        await client.get("/atlas/wfs?REQUEST=GetCapabilities&request=GetFeature")
    ).status_code == 400


async def test_capabilities_retain_proxy_prefix(atlas):
    app = FastAPI()
    app.include_router(atlas.router)
    async with AsyncClient(
        transport=ASGITransport(app, root_path="/proxy"), base_url="https://host"
    ) as client:
        response = await client.get("/proxy/atlas/wfs?request=GetCapabilities")
    assert response.status_code == 200 and "https://host/proxy/atlas/wfs" in response.text


@pytest.mark.parametrize("method", ["get", "post"])
@pytest.mark.parametrize(
    "sort,expected",
    [("nivel D", [2, 5, 1, 4, 3]), ("nivel D,id D", [5, 2, 4, 1, 3]), ("id", [1, 2, 3, 4, 5])],
)
async def test_sort_by_pages(client, method, sort, expected):
    ids = []
    for offset in (0, 2, 4):
        params = {
            "request": "GetFeature",
            "typeNames": "escuelas",
            "SoRtBy": sort,
            "COUNT": "2",
            "STARTINDEX": str(offset),
            "geometry": "false",
        }
        response = await (
            client.get("/atlas/wfs", params=params)
            if method == "get"
            else client.post("/atlas/wfs", data=params)
        )
        assert response.status_code == 200, response.text
        data = response.json()
        assert data["numberMatched"] == 5 and data["startIndex"] == offset
        ids.extend(f["id"] for f in data["features"])
    assert ids == expected and len(set(ids)) == 5


@pytest.mark.parametrize(
    "sort",
    [
        "",
        "id,",
        "geom",
        "missing A",
        "id DESC",
        "id A NULLS FIRST",
        "pg_sleep(1)",
        "id A,id D",
        "id;DROP TABLE x",
    ],
)
async def test_invalid_sort_by(client, sort):
    response = await client.get(
        "/atlas/wfs", params={"request": "GetFeature", "typeNames": "escuelas", "sortBy": sort}
    )
    assert response.status_code == 400 and response.json()["error"]["code"] == "invalid_filter"


async def test_duplicate_sort_by(client):
    response = await client.get(
        "/atlas/wfs?request=GetFeature&typeNames=escuelas&sortBy=id&SORTBY=id"
    )
    assert response.status_code == 400


SELECTION = {
    "op": "and",
    "args": [
        {
            "op": "or",
            "args": [
                {"op": "eq", "field": "nivel", "value": "primaria"},
                {"op": "is_null", "field": "nivel"},
            ],
        },
        {
            "op": "within",
            "srid": 4326,
            "geometry": {
                "type": "Polygon",
                "coordinates": [[[-103, 20], [-101, 20], [-101, 22], [-103, 22], [-103, 20]]],
            },
        },
    ],
}


@pytest.mark.parametrize("method", ["get", "post"])
async def test_atlas_filter_count_page_legacy_bbox_and_order(client, atlas, method):
    params = {
        "request": "GetFeature",
        "typeNames": "escuelas",
        "AtLaS_FiLtEr": json.dumps(SELECTION),
        "bbox": "-104,19,-100,23",
        "count": "1",
        "startIndex": "1",
        "sortBy": "id A",
        "geometry": "false",
    }
    db = atlas.context.db
    with (
        patch.object(db, "fetch_one", wraps=db.fetch_one) as count,
        patch.object(db, "stream", wraps=db.stream) as stream,
    ):
        response = await (
            client.get("/atlas/wfs", params=params)
            if method == "get"
            else client.post("/atlas/wfs", data=params)
        )
    assert response.status_code == 200, response.text
    data = response.json()
    assert data["numberMatched"] == 2 and data["numberReturned"] == 1
    assert [f["id"] for f in data["features"]] == [4]
    assert count.call_args.args[1] == stream.call_args.args[1][:-2]
    params.update({"id__gte": "2", "startIndex": "0"})
    response = await (
        client.get("/atlas/wfs", params=params)
        if method == "get"
        else client.post("/atlas/wfs", data=params)
    )
    assert response.status_code == 200 and response.json()["numberMatched"] == 1
    assert response.json()["features"][0]["id"] == 4


@pytest.mark.parametrize(
    "raw",
    [
        "{",
        '{"op":"eq","op":"ne","field":"id","value":1}',
        '{"op":"eq","field":"missing","value":1}',
        '{"op":"eq","field":"geom","value":1}',
        '{"op":"eq","field":"id","value":"1"}',
        '{"op":"union","field":"id","value":1}',
        json.dumps({"op": "within", "geometry": SELECTION["args"][1]["geometry"], "srid": 999999}),
    ],
)
async def test_invalid_atlas_filter_never_queries(client, atlas, monkeypatch, raw):
    monkeypatch.setattr(
        atlas.context.db, "fetch_one", AsyncMock(side_effect=AssertionError("unexpected SQL"))
    )
    monkeypatch.setattr(
        atlas.context.db, "stream", Mock(side_effect=AssertionError("unexpected cursor"))
    )
    response = await client.get(
        "/atlas/wfs", params={"request": "GetFeature", "typeNames": "escuelas", "ATLAS_FILTER": raw}
    )
    assert response.status_code == 400


@pytest.mark.parametrize("key", ["CQL_FILTER", "FiLtEr", "ATLAS_FILTERS"])
async def test_unsupported_filter_encoding_is_explicit(client, key):
    response = await client.get(
        "/atlas/wfs", params={"request": "GetFeature", "typeNames": "escuelas", key: ""}
    )
    assert response.status_code == 400 and response.json()["error"]["code"] == "invalid_filter"


async def test_atlas_filter_duplicates_across_query_and_form(client):
    response = await client.post(
        "/atlas/wfs?request=GetFeature&typeNames=escuelas&ATLAS_FILTER=null",
        data={"atlas_filter": "null"},
    )
    assert response.status_code == 400 and response.json()["error"]["code"] == "invalid_parameter"


@pytest.mark.parametrize(
    "limit,raw",
    [
        ({"filter_max_bytes": 2}, json.dumps(SELECTION)),
        ({"filter_max_depth": 1}, json.dumps(SELECTION)),
        ({"filter_max_nodes": 1}, json.dumps(SELECTION)),
        ({"filter_max_coordinates": 4}, json.dumps(SELECTION)),
        ({"filter_max_in_values": 1}, '{"op":"in","field":"id","value":[1,2]}'),
    ],
)
async def test_atlas_filter_enforces_config_before_sql(client, atlas, monkeypatch, limit, raw):
    atlas.context.config = atlas.context.config.model_copy(update=limit)
    monkeypatch.setattr(
        atlas.context.db, "fetch_one", AsyncMock(side_effect=AssertionError("unexpected SQL"))
    )
    response = await client.get(
        "/atlas/wfs", params={"request": "GetFeature", "typeNames": "escuelas", "atlas_filter": raw}
    )
    assert response.status_code == 400


async def test_atlas_filter_permissions_and_literal_value(client, atlas, monkeypatch):
    params = {
        "request": "GetFeature",
        "typeNames": "escuelas",
        "atlas_filter": json.dumps({"op": "eq", "field": "nombre", "value": "x' OR 1=1;--"}),
    }
    response = await client.get("/atlas/wfs", params=params)
    assert response.status_code == 200 and response.json()["numberMatched"] == 0
    atlas.context.permission_checker = lambda *args: False
    monkeypatch.setattr(
        atlas.context.db, "fetch_one", AsyncMock(side_effect=AssertionError("unexpected SQL"))
    )
    assert (await client.get("/atlas/wfs", params=params)).status_code == 403


async def test_python_get_feature_accepts_validated_predicate(atlas):
    from atlas.core.predicates import parse_predicate
    from atlas.wfs.get_feature import get_feature

    selection = parse_predicate('{"op":"eq","field":"id","value":2}', atlas.context.config)
    response = await get_feature(atlas.context, type_names="escuelas", predicate=selection)
    body = b""
    async for chunk in response.body_iterator:
        body += chunk.encode() if isinstance(chunk, str) else chunk
    data = json.loads(body)
    assert data["numberMatched"] == 1 and data["features"][0]["id"] == 2


@pytest.mark.parametrize("method", ["get", "post"])
async def test_atlas_filter_matches_real_nulls(client, atlas, method):
    from atlas.layers.metadata import build_layer

    async with atlas.context.db.connection() as conn:
        await conn.execute("""CREATE VIEW geo.wfs_nulls_test AS
            SELECT id, nombre, NULLIF(nivel,'media') AS nivel, geom FROM geo.escuelas""")
    try:
        atlas.register(await build_layer(atlas.context.db, "geo.wfs_nulls_test"))
        params = {
            "request": "GetFeature",
            "typeNames": "wfs_nulls_test",
            "atlas_filter": '{"op":"is_null","field":"nivel"}',
        }
        response = await (
            client.get("/atlas/wfs", params=params)
            if method == "get"
            else client.post("/atlas/wfs", data=params)
        )
        assert response.status_code == 200, response.text
        data = response.json()
        assert data["numberMatched"] == 1 and data["numberReturned"] == 1
        assert data["features"][0]["properties"]["id"] == 3
    finally:
        async with atlas.context.db.connection() as conn:
            await conn.execute("DROP VIEW geo.wfs_nulls_test")
