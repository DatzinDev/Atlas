"""Build layers from seeded tables and views, respecting explicit configuration."""

import pytest

from atlas.layers.metadata import build_layer
from atlas.postgis.connection import Database
from atlas.styles.models import PointStyle, PolygonStyle


async def test_table_defaults(db: Database) -> None:
    layer = await build_layer(db, "geo.municipios")
    assert (layer.name, layer.title, layer.schema_name, layer.table_name) == (
        "municipios",
        "Municipios",
        "geo",
        "municipios",
    )
    assert (layer.geometry_column, layer.geometry_type, layer.srid, layer.id_column) == (
        "geom",
        "Polygon",
        4326,
        "id",
    )
    assert layer.attribute_names == ["id", "clave", "nombre", "poblacion"]
    assert layer.extent == pytest.approx((-103, 20.5, -101.5, 21.5), abs=0.01)
    assert layer.style == PolygonStyle()


async def test_view_without_primary_key(db: Database) -> None:
    layer = await build_layer(db, "geo.v_municipios_grandes")
    assert layer.is_view and layer.id_column is None
    assert layer.extent == (-103, 20.5, -101.5, 21.5)


async def test_overrides_win(db: Database) -> None:
    layer = await build_layer(
        db,
        "geo.municipios",
        name="custom",
        title="Custom title",
        attributes=[],
        extent=None,
        style=None,
        downloadable=False,
    )
    assert (layer.name, layer.title) == ("custom", "Custom title")
    assert layer.attributes == [] and layer.extent is layer.style is None
    assert not layer.downloadable


async def test_explicit_style_and_geometry_override(db: Database) -> None:
    layer = await build_layer(db, "geo.escuelas", style=PointStyle(radius=10))
    assert layer.style == PointStyle(radius=10)
    changed = await build_layer(db, "geo.municipios", geometry_type="Point", extent=None)
    assert changed.style == PointStyle()


async def test_unknown_override_is_not_silently_ignored(db: Database) -> None:
    with pytest.raises(TypeError, match="titel"):
        await build_layer(db, "geo.municipios", titel="typo")
