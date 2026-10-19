"""Describe the public fields of a single feature type as JSON Schema."""

from typing import Any

from atlas.core.exceptions import UnsupportedFormat
from atlas.layers.models import Layer

PG_TO_JSON_TYPE = {
    "int2": "integer",
    "int4": "integer",
    "int8": "integer",
    "numeric": "number",
    "decimal": "number",
    "float4": "number",
    "float8": "number",
    "bool": "boolean",
    "text": "string",
    "varchar": "string",
    "bpchar": "string",
    "date": "string",
    "timestamp": "string",
    "timestamptz": "string",
    "uuid": "string",
    "json": "object",
    "jsonb": "object",
}


def describe_feature_type(layer: Layer, output_format: str = "application/json") -> dict[str, Any]:
    if output_format != "application/json":
        raise UnsupportedFormat("DescribeFeatureType supports application/json only")
    properties: dict[str, Any] = {}
    for column in layer.table_info.columns:
        kind = PG_TO_JSON_TYPE.get(column.data_type, "string")
        properties[column.name] = {"type": [kind, "null"] if column.nullable else kind}
        if column.data_type in ("date", "timestamp", "timestamptz"):
            properties[column.name]["format"] = (
                "date" if column.data_type == "date" else "date-time"
            )
    properties[layer.geometry_column] = {
        "type": ["object", "null"],
        "geometry_type": layer.geometry_type,
    }
    return {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "name": layer.name,
        "title": layer.title,
        "properties": properties,
        "geometry_column": layer.geometry_column,
        "geometry_type": layer.geometry_type,
        "srid": layer.srid,
    }
