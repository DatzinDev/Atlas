"""Valid UTF-8 GeoJSON, non-JSON values and memory independent of dataset size."""

import json
import tracemalloc
from datetime import date
from decimal import Decimal

import pytest

from atlas.exports.base import get_exporter
from atlas.exports.geojson import GeoJSONExporter
from atlas.layers.models import Layer


@pytest.fixture
def layer():
    return Layer(
        name="data",
        schema_name="geo",
        table_name="data",
        geometry_column="shape",
        geometry_type="Point",
        srid=4326,
        id_column="id",
    )


async def rows(count):
    for i in range(count):
        yield {
            "id": i,
            "shape": {"type": "Point", "coordinates": [1, 2]},
            "name": "México",
            "date": date(2026, 1, 1),
            "amount": Decimal("12.30"),
        }


@pytest.mark.parametrize("count", [0, 2])
async def test_valid_collection(layer, count):
    result = json.loads(
        b"".join([chunk async for chunk in GeoJSONExporter().export(rows(count), layer)])
    )
    assert len(result["features"]) == count and result["type"] == "FeatureCollection"
    assert result["crs"]["properties"]["name"] == "EPSG:4326"
    if count:
        feature = result["features"][0]
        assert feature["id"] == 0 and feature["geometry"]["coordinates"] == [1, 2]
        assert feature["properties"]["date"] == "2026-01-01"
        assert (
            feature["properties"]["amount"] == "12.30" and feature["properties"]["name"] == "México"
        )
        assert "shape" not in feature["properties"]
    assert isinstance(get_exporter("geojson"), GeoJSONExporter)


async def test_streaming_memory_does_not_grow_with_rows(layer):
    async def peak(count):
        tracemalloc.start()
        try:
            total = 0
            async for chunk in GeoJSONExporter().export(rows(count), layer):
                total += len(chunk)
            return total, tracemalloc.get_traced_memory()[1]
        finally:
            tracemalloc.stop()

    small_bytes, small_peak = await peak(100)
    large_bytes, large_peak = await peak(10000)
    assert large_bytes > small_bytes * 90
    assert large_peak < small_peak + 512000
