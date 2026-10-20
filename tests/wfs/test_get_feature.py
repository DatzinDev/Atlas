"""Bounded streaming, projection, error boundaries and cursor cleanup."""

import json
from unittest.mock import AsyncMock

import pytest
from starlette.requests import ClientDisconnect

from atlas.core.exceptions import InvalidFilter, LayerNotFound, PermissionDenied, ServiceDisabled
from atlas.wfs.filters import parse_filters
from atlas.wfs.get_feature import get_feature, parse_property_names


async def payload(response):
    return json.loads(
        "".join(
            [
                chunk if isinstance(chunk, str) else chunk.decode()
                async for chunk in response.body_iterator
            ]
        )
    )


async def test_geojson_page(atlas):
    response = await get_feature(atlas.context, type_names="municipios", count=2, start_index=1)
    data = await payload(response)
    assert response.media_type == "application/geo+json"
    assert data["type"] == "FeatureCollection" and data["numberMatched"] == 5
    assert data["numberReturned"] == 2 and data["startIndex"] == 1
    assert data["crs"]["properties"]["name"] == "EPSG:4326"
    assert all("id" in f and "geometry" in f and "properties" in f for f in data["features"])


async def test_utm_and_filtered_bbox(atlas):
    response = await get_feature(
        atlas.context, type_names="escuelas", bbox=(-102.9, 21.1, -102.5, 21.4), bbox_srid=4326
    )
    data = await payload(response)
    assert data["numberMatched"] == data["numberReturned"] == 1
    assert data["features"][0]["geometry"]["coordinates"] == pytest.approx([-102.8, 21.3], abs=0.01)
    filters = parse_filters({"poblacion__gt": "9999999"}, atlas.context.registry.get("municipios"))
    data = await payload(await get_feature(atlas.context, type_names="municipios", filters=filters))
    assert data["features"] == [] and data["numberReturned"] == 0


async def test_cursor_is_lazy_and_closed_on_early_exit(atlas, monkeypatch):
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
    response = await get_feature(atlas.context, type_names="municipios")
    assert len(consumed) == 1
    await anext(response.body_iterator)
    await response.body_iterator.aclose()
    assert closed == [True] and len(consumed) == 1


async def test_service_permission_and_missing_layer(atlas):
    with pytest.raises(LayerNotFound):
        await get_feature(atlas.context, type_names="missing")
    layer = atlas.context.registry.get("municipios")
    layer.services.wfs = False
    with pytest.raises(ServiceDisabled):
        await get_feature(atlas.context, type_names="municipios")
    layer.services.wfs = True
    atlas.context.permission_checker = lambda *args: False
    with pytest.raises(PermissionDenied):
        await get_feature(atlas.context, type_names="municipios")


async def test_disconnect_before_headers_releases_prefetched_cursor(atlas, monkeypatch):
    original = atlas.context.db.stream
    closed = []

    async def observed(*args, **kwargs):
        source = original(*args, **kwargs)
        try:
            async for row in source:
                yield row
        finally:
            await source.aclose()
            closed.append(True)

    monkeypatch.setattr(atlas.context.db, "stream", observed)
    response = await get_feature(atlas.context, type_names="municipios")

    async def disconnected(message):
        raise OSError("client disconnected before headers")

    async def receive():
        return {"type": "http.disconnect"}

    with pytest.raises((ClientDisconnect, ExceptionGroup)):
        await response({"type": "http", "asgi": {"spec_version": "2.4"}}, receive, disconnected)
    assert closed == [True]


@pytest.mark.parametrize(
    "properties,geometry,expected_geometry,expected_fields",
    [
        (None, None, True, {"id", "clave", "nombre", "poblacion"}),
        ("nombre", None, False, {"id", "nombre"}),
        ("nombre,geom", None, True, {"id", "nombre"}),
        ("geom", None, True, {"id"}),
        ("geom", "false", False, {"id"}),
    ],
)
async def test_http_property_projection_sql_and_geojson(
    client, atlas, monkeypatch, properties, geometry, expected_geometry, expected_fields
):
    statements = []
    original = atlas.context.db.stream

    def observed(query, values):
        statements.append(query.as_string())
        return original(query, values)

    monkeypatch.setattr(atlas.context.db, "stream", observed)
    params = {"request": "GetFeature", "typeNames": "municipios", "count": "1"}
    if properties is not None:
        params["propertyName"] = properties
    if geometry is not None:
        params["geometry"] = geometry
    response = await client.get("/atlas/wfs", params=params)
    assert response.status_code == 200
    feature = response.json()["features"][0]
    assert (feature["geometry"] is not None) == expected_geometry
    assert ("ST_AsGeoJSON" in statements[0]) == expected_geometry
    assert set(feature["properties"]) == expected_fields
    assert feature["id"] == feature["properties"]["id"]


async def test_custom_geometry_column_and_rejection_of_other_geometry(atlas, monkeypatch):
    layer = atlas.context.registry.get("municipios")
    layer.geometry_column = "shape"
    queries = []
    counter = AsyncMock(return_value={"count": 1})

    async def rows(query, values):
        queries.append(query.as_string())
        yield {"id": 1, "shape": {"type": "Point", "coordinates": [1, 2]}}

    monkeypatch.setattr(atlas.context.db, "fetch_one", counter)
    monkeypatch.setattr(atlas.context.db, "stream", rows)
    names = parse_property_names("shape", layer, allow_geometry=True)
    data = await payload(
        await get_feature(atlas.context, type_names=layer.name, property_names=names)
    )
    assert data["features"][0]["geometry"]["coordinates"] == [1, 2]
    assert 'AS "shape"' in queries[0] and '"geom"' not in queries[0]
    for name in ["geom", "other_geom", "pg_sleep(10)"]:
        with pytest.raises(InvalidFilter):
            await get_feature(atlas.context, type_names=layer.name, property_names=[name])
    assert counter.await_count == 1


async def test_download_properties_remain_attribute_only(client):
    ok = await client.get("/atlas/layers/municipios/download?properties=nombre")
    bad = await client.get("/atlas/layers/municipios/download?properties=geom")
    assert ok.status_code == 200
    assert bad.status_code == 400
