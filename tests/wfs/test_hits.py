"""Exact counts share filtering with results and never materialize feature output."""

import json
from datetime import datetime, timedelta
from unittest.mock import AsyncMock, Mock, patch
from xml.etree import ElementTree as ET

import pytest
from psycopg import sql

from atlas.layers.metadata import build_layer
from atlas.wfs.hits import get_hits


@pytest.mark.parametrize("total", [0, 1001, 10001])
async def test_hits_exact_beyond_page_caps(client, atlas, monkeypatch, total):
    async with atlas.context.db.connection() as conn:
        await conn.execute(
            sql.SQL("""CREATE VIEW geo.wfs_hits_test AS
            SELECT i AS id, ST_SetSRID(ST_MakePoint(0,0),4326)::geometry(Point,4326) AS geom
            FROM generate_series(1,{}) i""").format(sql.Literal(total))
        )
    try:
        atlas.register(await build_layer(atlas.context.db, "geo.wfs_hits_test"))
        monkeypatch.setattr(atlas.context.db, "stream", Mock(side_effect=AssertionError("cursor")))
        with patch.object(atlas.context.db, "fetch_one", wraps=atlas.context.db.fetch_one) as count:
            response = await client.get(
                "/atlas/wfs",
                params={
                    "request": "GetFeature",
                    "typeNames": "wfs_hits_test",
                    "resultType": "hits",
                    "count": "1",
                    "startIndex": "99999",
                },
            )
        assert response.status_code == 200, response.text
        root = ET.fromstring(response.content)
        assert root.tag == "{http://www.opengis.net/wfs/2.0}FeatureCollection" and len(root) == 0
        assert root.attrib["numberMatched"] == str(total) and root.attrib["numberReturned"] == "0"
        assert datetime.fromisoformat(root.attrib["timeStamp"]).utcoffset() == timedelta(0)
        query = count.call_args.args[0].as_string()
        assert "count(*)" in query
        assert all(
            word not in query for word in ("LIMIT", "OFFSET", "ST_AsGeoJSON", "ST_Transform")
        )
        assert ("next" in root.attrib) == (total > 0)
    finally:
        async with atlas.context.db.connection() as conn:
            await conn.execute("DROP VIEW geo.wfs_hits_test")


@pytest.mark.parametrize("method", ["get", "post"])
async def test_hits_matches_filtered_results_and_next(client, atlas, method):
    selection = {
        "op": "or",
        "args": [
            {"op": "in", "field": "id", "value": [1, 2, 4]},
            {"op": "is_null", "field": "nivel"},
        ],
    }
    selection = {
        "op": "and",
        "args": [
            selection,
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
    params = {
        "request": "GetFeature",
        "typeNames": "escuelas",
        "ATLAS_FILTER": json.dumps(selection),
        "id__gte": "2",
        "bbox": "-104,19,-100,23",
    }
    results = (await client.get("/atlas/wfs", params=params)).json()
    params["resultType"] = "hits"
    response = await (
        client.get("/atlas/wfs", params=params)
        if method == "get"
        else client.post("/atlas/wfs", data=params)
    )
    assert response.status_code == 200 and response.headers["content-type"].startswith(
        "application/xml"
    )
    root = ET.fromstring(response.content)
    assert int(root.attrib["numberMatched"]) == results["numberMatched"] == 2
    assert (await client.get(root.attrib["next"])).json()["numberMatched"] == 2
    assert await get_hits(atlas.context, type_names="escuelas") == 5


async def test_hits_permission_service_and_invalid_filter_before_sql(client, atlas, monkeypatch):
    monkeypatch.setattr(atlas.context.db, "fetch_one", AsyncMock(side_effect=AssertionError("SQL")))
    params = {"request": "GetFeature", "typeNames": "escuelas", "resultType": "hits"}
    atlas.context.permission_checker = lambda *args: False
    assert (await client.get("/atlas/wfs", params=params)).status_code == 403
    atlas.context.permission_checker = None
    for raw in ("{", '{"op":"eq","field":"missing","value":1}'):
        assert (
            await client.get("/atlas/wfs", params={**params, "atlas_filter": raw})
        ).status_code == 400
    for extra in ({"CQL_FILTER": ""}, {"resultType": "bad"}, {"outputFormat": "application/json"}):
        assert (await client.get("/atlas/wfs", params={**params, **extra})).status_code == 400
    layer = atlas.context.registry.get("escuelas")
    layer.services = layer.services.model_copy(update={"wfs": False})
    assert (await client.get("/atlas/wfs", params=params)).status_code == 404
