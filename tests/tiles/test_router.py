"""Native tiles through the integrated router and host dependency boundary."""

import json
from unittest.mock import AsyncMock

import mapbox_vector_tile
import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient

from atlas import Atlas


async def test_native_tile_empty_permissions_and_filter(client, atlas):
    layer = atlas.context.registry.get("municipios")
    path = "/atlas/tiles/municipios/6/13/28.mvt"
    assert (await client.get(path)).status_code == 404
    layer.services.mvt = True
    response = await client.get(
        path,
        params=dict(
            properties="nombre", ATLAS_FILTER=json.dumps({"op": "eq", "field": "id", "value": 1})
        ),
    )
    assert response.status_code == 200
    assert response.headers["content-type"] == "application/vnd.mapbox-vector-tile"
    assert response.headers["cache-control"] == "no-store"
    assert mapbox_vector_tile.decode(response.content)["municipios"]["features"][0][
        "properties"
    ] == {"nombre": "San Bernardo"}
    response = await client.get("/atlas/tiles/municipios/6/0/0.mvt")
    assert response.status_code == 200 and response.content == b""
    atlas.context.permission_checker = lambda layer, op, user: op != "query"
    assert (await client.get(path)).status_code == 403


@pytest.mark.parametrize(
    "suffix",
    [
        "-1/0/0.mvt",
        "1/2/0.mvt",
        "23/0/0.mvt",
        "1.0/0/0.mvt",
        "1/0/0.mvt?properties=",
        "1/0/0.mvt?properties=id,id",
        "1/0/0.mvt?CQL_FILTER=1",
        "1/0/0.mvt?ATLAS_FILTER={}",
        "1/0/0.mvt?properties=id&Properties=nombre",
    ],
)
async def test_invalid_input_before_service(client, monkeypatch, suffix):
    service = AsyncMock(side_effect=AssertionError("invalid input reached service"))
    monkeypatch.setattr("atlas.tiles.router.get_vector_tile", service)
    response = await client.get("/atlas/tiles/municipios/" + suffix)
    assert response.status_code == 400
    service.assert_not_awaited()


async def test_prefix_and_principal_dependency(atlas, database_url, monkeypatch):
    service = AsyncMock(return_value=b"")
    monkeypatch.setattr("atlas.tiles.router.get_vector_tile", service)
    geo = Atlas(database_url, prefix="/geo", principal_dependency=lambda: "reader")
    app = FastAPI()
    app.include_router(geo.router)
    async with AsyncClient(transport=ASGITransport(app), base_url="https://host") as client:
        response = await client.get("/geo/tiles/municipios/0/0/0.mvt")
    assert response.status_code == 200
    assert service.call_args.kwargs["principal"] == "reader"
