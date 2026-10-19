"""Layer validation, service flags and the SQL attribute whitelist."""

import pytest
from pydantic import ValidationError

from atlas.layers.models import Layer, LayerServices
from atlas.postgis.introspection import ColumnInfo, TableInfo


def make_layer(**overrides: object) -> Layer:
    return Layer.model_validate(
        {
            "name": "municipios",
            "schema_name": "geo",
            "table_name": "municipios",
            "geometry_column": "geom",
            "geometry_type": "Polygon",
            "srid": 4326,
            **overrides,
        }
    )


@pytest.mark.parametrize("name", ["municipios", "layer_2", "layer-name", "123"])
def test_url_safe_names(name: str) -> None:
    assert make_layer(name=name).name == name


@pytest.mark.parametrize("name", ["", "Municipios", "geo.municipios", "a/b", "a b", "á", "a\n"])
def test_invalid_names(name: str) -> None:
    with pytest.raises(ValidationError):
        make_layer(name=name)


def test_title_default_and_override() -> None:
    assert make_layer().title == "Municipios"
    assert make_layer(title="Municipios de Jalisco").title == "Municipios de Jalisco"


def test_defaults_are_independent() -> None:
    first, second = make_layer(), make_layer()
    first.permissions.append("atlas.municipios.view")
    first.attributes.append(ColumnInfo(name="id", data_type="int4", nullable=False))
    first.services.wms = False
    assert second.permissions == second.attributes == []
    assert second.services == LayerServices()
    assert second.enabled and second.searchable and second.downloadable and second.public
    assert second.style is second.extent is None


def test_geometry_is_excluded_from_attributes_and_whitelist() -> None:
    geometry = ColumnInfo(name="geom", data_type="geometry", nullable=False)
    layer = make_layer(
        attributes=[geometry, ColumnInfo(name="id", data_type="int4", nullable=False)]
    )
    assert [a.name for a in layer.attributes] == ["id"]
    layer.attributes.append(geometry)
    assert layer.attribute_names == ["id"]
    assert [a.name for a in layer.table_info.columns] == ["id"]


@pytest.mark.parametrize("is_view", [False, True])
def test_table_info_round_trip(is_view: bool) -> None:
    info = TableInfo(
        schema_name="geo",
        table_name="municipios",
        geometry_column="geom",
        geometry_type="Polygon",
        srid=4326,
        columns=[ColumnInfo(name="nombre", data_type="text", nullable=True)],
        primary_key=None if is_view else "id",
        is_view=is_view,
    )
    layer = make_layer(attributes=info.columns, id_column=info.primary_key, is_view=info.is_view)
    assert layer.table_info == info


@pytest.mark.parametrize("service", ["wms", "wfs", "wmts", "wcs"])
def test_supports_requires_enabled_layer_and_service(service: str) -> None:
    layer = make_layer(services={service: True})
    assert layer.supports(service)
    setattr(layer.services, service, False)
    assert not layer.supports(service)
    setattr(layer.services, service, True)
    layer.enabled = False
    assert not layer.supports(service)


def test_service_defaults_and_unknown_names() -> None:
    layer = make_layer()
    assert layer.supports("wms") and layer.supports("wfs")
    assert not layer.supports("wmts") and not layer.supports("wcs")
    assert not layer.supports("model_dump") and not layer.supports("unknown")


def test_unknown_geometry_is_rejected() -> None:
    with pytest.raises(ValidationError):
        make_layer(geometry_type="NotAGeometry")


def test_styles_are_validated() -> None:
    assert make_layer(style={"fill": "#ffffff"}).style.fill == "#ffffff"
    with pytest.raises(ValidationError):
        make_layer(style={"fill": "not a color"})
