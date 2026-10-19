"""Public catalog and details expose only enabled layer metadata."""

import pytest
from fastapi import FastAPI, Header
from httpx import ASGITransport, AsyncClient

from atlas import Atlas


async def test_catalog_excludes_attributes_and_disabled_layers(client: AsyncClient, atlas: Atlas):
    atlas.context.registry.get("carreteras").enabled = False
    response = await client.get("/atlas/layers")
    assert response.status_code == 200
    layers = response.json()["layers"]
    assert [layer["name"] for layer in layers] == ["escuelas", "municipios"]
    assert all("attributes" not in layer and "schema_name" not in layer for layer in layers)


async def test_layer_detail(client: AsyncClient) -> None:
    response = await client.get("/atlas/layers/municipios")
    assert response.status_code == 200
    detail = response.json()
    assert detail == {
        "name": "municipios",
        "title": "Municipios",
        "description": None,
        "geometry_type": "Polygon",
        "srid": 4326,
        "extent": [-103, 20.5, -101.5, 21.5],
        "attributes": [
            {"name": "id", "type": "int4"},
            {"name": "clave", "type": "text"},
            {"name": "nombre", "type": "text"},
            {"name": "poblacion", "type": "int4"},
        ],
        "services": {"wms": True, "wfs": True, "wmts": False, "wcs": False},
        "download_formats": ["csv", "geojson"],
        "style": {
            "fill": "#3178c6",
            "fill_opacity": 0.7,
            "stroke": "#ffffff",
            "stroke_width": 1.0,
            "stroke_opacity": 1.0,
        },
        "min_zoom": None,
        "max_zoom": None,
    }


async def test_downloads_disabled(client: AsyncClient, atlas: Atlas) -> None:
    atlas.context.registry.get("municipios").downloadable = False
    assert (await client.get("/atlas/layers/municipios")).json()["download_formats"] == []


async def test_unknown_layer_uses_atlas_error(client: AsyncClient) -> None:
    response = await client.get("/atlas/layers/missing")
    assert response.status_code == 404
    assert response.json() == {
        "error": {
            "code": "layer_not_found",
            "message": "the requested layer is not registered",
            "details": {"name": "missing"},
        }
    }


async def test_disabled_layer_is_not_served(client: AsyncClient, atlas: Atlas) -> None:
    atlas.context.registry.get("municipios").enabled = False
    response = await client.get("/atlas/layers/municipios")
    assert response.status_code == 404
    assert response.json()["error"]["code"] == "layer_not_found"


async def test_catalog_openapi(client: AsyncClient) -> None:
    schemas = (await client.get("/openapi.json")).json()["components"]["schemas"]
    assert "LayerSummary" in schemas and "LayerDetailResponse" in schemas


@pytest.mark.parametrize("asynchronous", [False, True])
async def test_host_principal_filters_catalog_and_direct_access(atlas: Atlas, asynchronous: bool):
    def principal(x_user: str = Header(default="anonymous")):
        return x_user

    def checker(layer, op, user):
        return user == "allowed" and layer.name == "municipios" and op in ("view", "query")

    async def async_checker(layer, op, user):
        return checker(layer, op, user)

    atlas.context.permission_checker = async_checker if asynchronous else checker
    atlas.context.principal_dependency = principal
    app = FastAPI()
    app.include_router(atlas.router)
    async with AsyncClient(transport=ASGITransport(app), base_url="http://test") as client:
        assert (await client.get("/atlas/layers")).json() == {"layers": []}
        denied = await client.get("/atlas/layers/municipios")
        assert denied.status_code == 403
        assert denied.json()["error"]["code"] == "permission_denied"
        headers = {"x-user": "allowed"}
        listing = (await client.get("/atlas/layers", headers=headers)).json()
        assert [layer["name"] for layer in listing["layers"]] == ["municipios"]
        assert (await client.get("/atlas/layers/municipios", headers=headers)).status_code == 200


async def test_checker_failure_is_closed_over_http(client: AsyncClient, atlas: Atlas) -> None:
    def broken(*args):
        raise RuntimeError("secret database password")

    atlas.context.permission_checker = broken
    assert (await client.get("/atlas/layers")).json() == {"layers": []}
    response = await client.get("/atlas/layers/municipios")
    assert response.status_code == 403 and "secret" not in response.text


async def test_detail_checks_query_independently_of_catalog_view(client: AsyncClient, atlas: Atlas):
    atlas.context.permission_checker = lambda layer, op, principal: op == "query"
    assert (await client.get("/atlas/layers")).json() == {"layers": []}
    assert (await client.get("/atlas/layers/municipios")).status_code == 200
