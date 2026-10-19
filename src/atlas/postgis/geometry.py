"""Convert a dict row into a GeoJSON feature without changing its source."""

from typing import Any


def to_geojson_feature(
    row: dict[str, Any], id_column: str | None, geometry_column: str = "geom"
) -> dict[str, Any]:
    feature = {
        "type": "Feature",
        "geometry": row.get(geometry_column),
        "properties": {key: value for key, value in row.items() if key != geometry_column},
    }
    if id_column is not None and id_column in row:
        feature["id"] = row[id_column]
    return feature
