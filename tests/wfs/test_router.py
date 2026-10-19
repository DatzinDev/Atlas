"""HTTP dispatch, validation, auth, logging and proxy-aware capabilities."""

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
