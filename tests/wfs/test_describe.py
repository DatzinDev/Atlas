"""Feature schemas identify geometry and translate database field types."""

import pytest

from atlas.core.exceptions import UnsupportedFormat
from atlas.postgis.introspection import ColumnInfo
from atlas.wfs.describe import describe_feature_type


async def test_schema(municipios_layer):
    result = describe_feature_type(municipios_layer)
    assert result["name"] == "municipios" and result["srid"] == 4326
    assert result["geometry_column"] == "geom" and result["geometry_type"] == "Polygon"
    assert result["properties"]["poblacion"]["type"] == "integer"
    assert result["properties"]["nombre"]["type"] == "string"
    assert result["properties"]["geom"]["geometry_type"] == "Polygon"
    with pytest.raises(UnsupportedFormat):
        describe_feature_type(municipios_layer, "text/xml")


async def test_nullable_and_unknown_types(municipios_layer):
    municipios_layer.attributes = [
        ColumnInfo(name="custom", data_type="unknown", nullable=True),
        ColumnInfo(name="date", data_type="date", nullable=False),
    ]
    result = describe_feature_type(municipios_layer)
    assert result["properties"]["custom"] == {"type": ["string", "null"]}
    assert result["properties"]["date"]["format"] == "date"
