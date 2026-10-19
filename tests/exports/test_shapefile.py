"""Shapefile components, field mapping, unsupported types, limits and cleanup."""

from datetime import UTC, date, datetime
from io import BytesIO
from tempfile import TemporaryDirectory
from zipfile import ZipFile

import pytest

from atlas.core.exceptions import InvalidGeometry, InvalidParameter, QueryTooLarge
from atlas.exports.shapefile import ShapefileExporter
from atlas.layers.models import Layer
from atlas.postgis.introspection import ColumnInfo

fiona = pytest.importorskip("fiona")


@pytest.fixture
def layer():
    types = {
        "nombre_municipal": "text",
        "nombre_municipio": "text",
        "Nombre_municipal": "text",
        "instant": "timestamptz",
        "members": "_int4",
        "big": "int8",
        "score": "float8",
        "day": "date",
        "active": "bool",
        "count": "int4",
    }
    return Layer(
        name="points",
        schema_name="geo",
        table_name="points",
        geometry_column="geom",
        geometry_type="Point",
        srid=4326,
        attributes=[ColumnInfo(name=k, data_type=v, nullable=True) for k, v in types.items()],
    )


async def rows(count=2):
    for _ in range(count):
        yield {
            "nombre_municipal": "México",
            "nombre_municipio": "Norte",
            "Nombre_municipal": "Sur",
            "instant": datetime(2026, 1, 1, tzinfo=UTC),
            "members": [1, 2],
            "big": 2**60,
            "score": 1.5,
            "day": date(2026, 1, 1),
            "active": True,
            "count": 3,
            "geom": {"type": "Point", "coordinates": [-102.8, 21.3]},
        }


@pytest.mark.parametrize("count", [0, 2])
async def test_roundtrip_components_field_mapping_crs_and_types(layer, tmp_path, count):
    data = b"".join([chunk async for chunk in ShapefileExporter().export(rows(count), layer)])
    with ZipFile(BytesIO(data)) as bundle:
        assert set(bundle.namelist()) == {
            "points." + ext for ext in ("shp", "shx", "dbf", "prj", "cpg")
        } | {"README.txt"}
        assert bundle.read("points.cpg") == b"UTF-8"
        assert fiona.crs.CRS.from_wkt(bundle.read("points.prj").decode()).to_epsg() == 4326
        readme = bundle.read("README.txt").decode()
        assert "nombre_municipal -> nombre_mun" in readme
        assert "nombre_municipio -> nombre_m_1" in readme
        bundle.extractall(tmp_path)
    with fiona.open(tmp_path / "points.shp") as source:
        assert len(source) == count and source.crs.to_epsg() == 4326
        fields = list(source.schema["properties"])
        assert all(len(name) <= 10 for name in fields) and len(
            set(name.lower() for name in fields)
        ) == len(fields)
        if count:
            feature = next(iter(source))
            assert feature["properties"]["nombre_mun"] == "México"
            assert feature["properties"]["instant"] == "2026-01-01T00:00:00+00:00"
            assert feature["properties"]["members"] == "[1, 2]"
            assert feature["properties"]["big"] == str(2**60)
            assert feature["properties"]["score"] == 1.5
            assert feature["properties"]["day"] == "2026-01-01"
            assert feature["properties"]["active"] == 1
            assert feature["properties"]["count"] == 3


async def test_cleanup_and_limits(layer, tmp_path, monkeypatch):
    monkeypatch.setattr(
        "atlas.exports.shapefile.TemporaryDirectory",
        lambda **kwargs: TemporaryDirectory(dir=tmp_path, **kwargs),
    )
    stream = ShapefileExporter().export(rows(), layer)
    await anext(stream)
    assert list(tmp_path.iterdir())
    await stream.aclose()
    assert list(tmp_path.iterdir()) == []
    wide = layer.model_copy(
        update={
            "attributes": [
                ColumnInfo(name=f"f{i}", data_type="text", nullable=True) for i in range(256)
            ]
        }
    )
    with pytest.raises(QueryTooLarge, match="255"):
        await anext(ShapefileExporter().export(rows(), wide))
    monkeypatch.setattr("atlas.exports.shapefile.MAX_BYTES", 10)
    with pytest.raises(QueryTooLarge, match="2 GB"):
        await anext(ShapefileExporter().export(rows(), layer))
    assert list(tmp_path.iterdir()) == []


async def test_unrepresentable_data_and_source_error_cleanup(layer, tmp_path, monkeypatch):
    monkeypatch.setattr(
        "atlas.exports.shapefile.TemporaryDirectory",
        lambda **kwargs: TemporaryDirectory(dir=tmp_path, **kwargs),
    )

    async def bad(changes):
        yield (await anext(rows())) | changes

    with pytest.raises(InvalidParameter, match="254"):
        await anext(ShapefileExporter().export(bad({"nombre_municipal": "á" * 128}), layer))
    with pytest.raises(InvalidParameter, match="numeric range"):
        await anext(ShapefileExporter().export(bad({"score": 1e40}), layer))
    with pytest.raises(InvalidGeometry, match="mixed"):
        await anext(
            ShapefileExporter().export(
                bad({"geom": {"type": "LineString", "coordinates": [[0, 0], [1, 1]]}}), layer
            )
        )

    async def failed():
        yield await anext(rows())
        raise RuntimeError("source failed")

    with pytest.raises(RuntimeError, match="source failed"):
        await anext(ShapefileExporter().export(failed(), layer))
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize(
    "kind,coordinates",
    [
        ("Point", [0, 0]),
        ("MultiPoint", [[0, 0], [1, 1]]),
        ("LineString", [[0, 0], [1, 1]]),
        ("MultiLineString", [[[0, 0], [1, 1]], [[2, 2], [3, 3]]]),
        ("Polygon", [[[0, 0], [1, 0], [1, 1], [0, 0]]]),
        ("MultiPolygon", [[[[0, 0], [1, 0], [1, 1], [0, 0]]]]),
    ],
)
async def test_geometry_families(layer, kind, coordinates, tmp_path):
    async def source():
        yield (await anext(rows())) | {"geom": {"type": kind, "coordinates": coordinates}}

    metadata = layer.model_copy(update={"geometry_type": kind})
    data = b"".join([chunk async for chunk in ShapefileExporter().export(source(), metadata)])
    with ZipFile(BytesIO(data)) as archive:
        archive.extractall(tmp_path)
    with fiona.open(tmp_path / "points.shp") as result:
        assert len(result) == 1 and next(iter(result))["geometry"] is not None
