"""Requested attributes are whitelisted while feature identifiers remain available."""

import json

import pytest

from atlas.core.exceptions import InvalidFilter
from atlas.wfs.get_feature import get_feature, parse_property_names


@pytest.mark.parametrize("value", [None, "", "   "])
async def test_empty_means_all(municipios_layer, value):
    assert parse_property_names(value, municipios_layer) is None


@pytest.mark.parametrize("value", ["missing", "pg_sleep(10)", "geom", "nombre;DROP TABLE"])
async def test_invalid_properties(municipios_layer, value):
    with pytest.raises(InvalidFilter) as error:
        parse_property_names(value, municipios_layer)
    assert error.value.details["column"] == value


async def test_selected_fields_id_and_optional_geometry(atlas):
    layer = atlas.context.registry.get("municipios")
    columns = parse_property_names(" nombre , clave ,nombre", layer)
    assert columns == ["nombre", "clave"]
    for include_geometry in (True, False):
        response = await get_feature(
            atlas.context,
            type_names="municipios",
            property_names=columns,
            include_geometry=include_geometry,
            count=1,
        )
        data = json.loads("".join([chunk async for chunk in response.body_iterator]))
        feature = data["features"][0]
        assert set(feature["properties"]) == {"nombre", "clave", "id"}
        assert feature["id"] == feature["properties"]["id"]
        assert (feature["geometry"] is not None) == include_geometry


async def test_direct_property_list_is_also_checked(atlas):
    with pytest.raises(InvalidFilter):
        await get_feature(atlas.context, type_names="municipios", property_names=["pg_sleep(10)"])
