"""GeoPackage round trips types/CRS and removes temporary files on every exit."""

from datetime import UTC, date, datetime
from tempfile import TemporaryDirectory

import pytest

from atlas.exports.geopackage import GeoPackageExporter
from atlas.layers.models import Layer
from atlas.postgis.introspection import ColumnInfo

fiona = pytest.importorskip("fiona")


@pytest.fixture
def layer():
    types = {
        "fid": "int8",
        "name": "text",
        "active": "bool",
        "count": "int4",
        "ratio": "float8",
        "day": "date",
        "instant": "timestamptz",
        "exact": "numeric",
    }
    return Layer(
        name="points",
        schema_name="geo",
        table_name="points",
        geometry_column="shape",
        geometry_type="Point",
        srid=32613,
        attributes=[ColumnInfo(name=k, data_type=v, nullable=True) for k, v in types.items()],
    )


async def rows(count=2):
    for index in range(count):
        yield {
            "fid": 2**40 + index,
            "name": "México",
            "active": True,
            "count": 3,
            "ratio": 1.5,
            "day": date(2026, 1, 1),
            "instant": datetime(2026, 1, 1, tzinfo=UTC),
            "exact": "12345678901234567890.12345",
            "shape": {"type": "Point", "coordinates": [728225, 2356942]},
        }


@pytest.mark.parametrize("count", [0, 2])
async def test_roundtrip_types_crs_and_layer(layer, tmp_path, count):
    path = tmp_path / "points.gpkg"
    with path.open("wb") as target:
        async for chunk in GeoPackageExporter().export(rows(count), layer):
            target.write(chunk)
    with fiona.open(path, layer=layer.name) as source:
        assert len(source) == count and source.crs.to_epsg() == 32613
        assert source.schema["properties"]["fid"] == "int"
        assert source.schema["properties"]["count"] == "int32"
        assert source.schema["properties"]["day"] == "date"
        assert source.schema["properties"]["instant"] == "datetime"
        if count:
            feature = next(iter(source))
            assert feature["properties"]["fid"] == 2**40
            assert (
                feature["properties"]["name"] == "México"
                and feature["properties"]["active"] is True
            )
            assert feature["geometry"]["coordinates"] == (728225, 2356942)


async def test_temporary_cleanup_on_abort_and_source_error(layer, monkeypatch, tmp_path):
    def directory(**kwargs):
        return TemporaryDirectory(dir=tmp_path, **kwargs)

    monkeypatch.setattr("atlas.exports.geopackage.TemporaryDirectory", directory)
    stream = GeoPackageExporter().export(rows(), layer)
    await anext(stream)
    assert list(tmp_path.iterdir())
    await stream.aclose()
    assert list(tmp_path.iterdir()) == []

    async def failed():
        yield await anext(rows())
        raise RuntimeError("source failed")

    with pytest.raises(RuntimeError, match="source failed"):
        await anext(GeoPackageExporter().export(failed(), layer))
    assert list(tmp_path.iterdir()) == []
