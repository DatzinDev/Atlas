"""HTTP WMS dispatch, host integration, error boundaries, permissions and operation reports."""

from io import BytesIO
from unittest.mock import AsyncMock
from xml.etree import ElementTree as ET

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from PIL import Image

from atlas import Atlas
from atlas.rendering.pillow import PillowRenderer

MAP = dict(
    REQUEST="GetMap",
    LAYERS="municipios",
    BBOX="20.4,-103.1,21.6,-101.4",
    CRS="EPSG:4326",
    WIDTH="100",
    HEIGHT="100",
    FORMAT="image/png",
    STYLES="",
    TRANSPARENT="true",
)
NS = {"w": "http://www.opengis.net/wms"}


@pytest.mark.parametrize("key", ["REQUEST", "request", "Request"])
async def test_capabilities_case_and_default_version(client, key):
    response = await client.get("/atlas/wms", params={key: "GetCapabilities", "SeRvIcE": "WMS"})
    assert response.status_code == 200
    assert response.headers["content-type"].startswith("application/xml")
    root = ET.fromstring(response.text)
    assert root.attrib["version"] == "1.3.0"
    assert len(root.findall("w:Capability/w:Layer/w:Layer", NS)) == 3


async def test_map_info_and_operation_reports(client, atlas):
    reports = []
    atlas.context.log_hook = reports.append
    assert isinstance(atlas.context.renderer, PillowRenderer)
    response = await client.get("/atlas/wms", params=MAP)
    assert response.status_code == 200 and response.headers["content-type"] == "image/png"
    assert Image.open(BytesIO(response.content)).size == (100, 100)
    assert reports[-1].operation == "GetMap" and reports[-1].feature_count == 5
    assert reports[-1].layer == "municipios" and reports[-1].execution_time > 0
    info = MAP | dict(
        REQUEST="GetFeatureInfo", QUERY_LAYERS="municipios", I="25", J="25", GEOMETRY="true"
    )
    response = await client.get("/atlas/wms", params=info)
    assert response.status_code == 200 and response.headers["content-type"] == "application/json"
    assert response.json()["features"][0]["geometry"]["type"] == "Polygon"
    assert reports[-1].operation == "GetFeatureInfo" and reports[-1].feature_count == 1
    await client.get("/atlas/wms?REQUEST=GetCapabilities")
    assert reports[-1].feature_count == 3 and reports[-1].error is None


@pytest.mark.parametrize(
    "params,code",
    [
        ({"REQUEST": "unknown"}, "invalid_parameter"),
        ({"REQUEST": "GetCapabilities", "SERVICE": "WFS"}, "invalid_parameter"),
        ({"REQUEST": "GetCapabilities", "VERSION": "1.1.1"}, "invalid_parameter"),
        (MAP | {"FORMAT": "image/gif"}, "unsupported_format"),
        (MAP | {"WIDTH": "20000"}, "invalid_parameter"),
        (MAP | {"HEIGHT": "-1"}, "invalid_parameter"),
        (MAP | {"BBOX": "nan,0,1,1"}, "invalid_parameter"),
        (MAP | {"BBOX": "not-a-bbox"}, "invalid_parameter"),
        (MAP | {"CRS": "EPSG:1234"}, "unsupported_crs"),
        (
            MAP | {"REQUEST": "GetFeatureInfo", "QUERY_LAYERS": "municipios", "I": "100", "J": "0"},
            "invalid_parameter",
        ),
        (
            MAP
            | {
                "REQUEST": "GetFeatureInfo",
                "QUERY_LAYERS": "municipios",
                "I": "0",
                "J": "0",
                "INFO_FORMAT": "text/html",
            },
            "unsupported_format",
        ),
    ],
)
async def test_errors_are_json_and_logged(client, atlas, params, code):
    reports = []
    atlas.context.log_hook = reports.append
    response = await client.get("/atlas/wms", params=params)
    assert response.status_code == 400 and response.json()["error"]["code"] == code
    assert reports[-1].error == code


async def test_permission_filtering_and_service_flags(client, atlas):
    atlas.context.permission_checker = lambda layer, op, user: layer.name == "escuelas"
    response = await client.get("/atlas/wms?REQUEST=GetCapabilities")
    root = ET.fromstring(response.text)
    assert [node.text for node in root.findall("w:Capability/w:Layer/w:Layer/w:Name", NS)] == [
        "escuelas"
    ]
    assert (await client.get("/atlas/wms", params=MAP)).status_code == 403
    atlas.context.permission_checker = None
    atlas.context.registry.get("municipios").services.wms = False
    assert (await client.get("/atlas/wms", params=MAP)).status_code == 404
    assert (await client.get("/atlas/wms", params=MAP | {"LAYERS": "missing"})).status_code == 404
    assert (
        await client.get("/atlas/wms?REQUEST=GetMap&request=GetCapabilities")
    ).status_code == 400


async def test_custom_prefix_principal_and_proxy(atlas, database_url):
    seen = []
    geo = Atlas(
        database_url,
        prefix="/geo",
        principal_dependency=lambda: "reader",
        permission_checker=lambda layer, op, user: seen.append(user) or True,
    )
    for layer in atlas.context.registry.list():
        geo.register(layer)
    app = FastAPI()
    app.include_router(geo.router)
    async with AsyncClient(
        transport=ASGITransport(app, root_path="/proxy"), base_url="https://host"
    ) as client:
        response = await client.get("/proxy/geo/wms?request=GetCapabilities")
    assert response.status_code == 200 and "https://host/proxy/geo/wms" in response.text
    assert seen == ["reader"] * 3


@pytest.mark.parametrize("operation", ["GetMap", "GetFeatureInfo"])
@pytest.mark.parametrize(
    "name",
    ["CQL_FILTER", "TIME", "ENV", "SLD", "SLD_BODY", "FILTER", "ATLAS_FILTER"],
)
@pytest.mark.parametrize("value", ["", "unsupported"])
async def test_unsupported_content_fails_before_pipeline(
    client, monkeypatch, operation, name, value
):
    service = AsyncMock(
        side_effect=AssertionError("content reached the data/render/cache pipeline")
    )
    target = "get_map" if operation == "GetMap" else "get_feature_info"
    monkeypatch.setattr(f"atlas.wms.router.{target}", service)
    params = MAP | dict(REQUEST=operation, QUERY_LAYERS="municipios", I="0", J="0")
    response = await client.get("/atlas/wms", params=params | {name.swapcase(): value})
    assert response.status_code == 400
    assert response.json()["error"]["code"] == "invalid_parameter"
    service.assert_not_awaited()


@pytest.mark.parametrize("operation", ["GetMap", "GetFeatureInfo"])
async def test_named_style_fails_before_pipeline(client, monkeypatch, operation):
    service = AsyncMock(side_effect=AssertionError("named style reached the pipeline"))
    target = "get_map" if operation == "GetMap" else "get_feature_info"
    monkeypatch.setattr(f"atlas.wms.router.{target}", service)
    response = await client.get("/atlas/wms", params=MAP | dict(REQUEST=operation, STYLES="named"))
    assert response.status_code == 400
    service.assert_not_awaited()


@pytest.mark.parametrize("styles", ["", ","])
async def test_default_styles_and_transport_parameters(client, styles):
    response = await client.get(
        "/atlas/wms",
        params=MAP | dict(LAYERS="municipios,carreteras", STYLES=styles, TILED="true", _="123"),
    )
    assert response.status_code == 200
    assert response.headers["content-type"] == "image/png"
