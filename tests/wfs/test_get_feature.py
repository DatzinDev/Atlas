"""Bounded streaming, projection, error boundaries and cursor cleanup."""

import json

import pytest
from starlette.requests import ClientDisconnect

from atlas.core.exceptions import LayerNotFound, PermissionDenied, ServiceDisabled
from atlas.wfs.filters import parse_filters
from atlas.wfs.get_feature import get_feature


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
