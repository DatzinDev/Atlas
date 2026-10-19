"""Write GeoJSON one feature at a time without materializing the source iterator."""

from __future__ import annotations

import json
from collections.abc import AsyncIterator
from typing import TYPE_CHECKING, Any, ClassVar

if TYPE_CHECKING:
    from atlas.layers.models import Layer


class GeoJSONExporter:
    format: ClassVar[str] = "geojson"
    media_type: ClassVar[str] = "application/geo+json"
    extension: ClassVar[str] = "geojson"
    streaming: ClassVar[bool] = True

    async def export(
        self, rows: AsyncIterator[dict[str, Any]], layer: Layer
    ) -> AsyncIterator[bytes]:
        crs = {"type": "name", "properties": {"name": f"EPSG:{layer.srid}"}}
        yield ('{"type":"FeatureCollection","crs":' + json.dumps(crs) + ',"features":[').encode()
        separator = b""
        async for row in rows:
            feature = {
                "type": "Feature",
                "geometry": row.get(layer.geometry_column),
                "properties": {
                    key: value for key, value in row.items() if key != layer.geometry_column
                },
            }
            if layer.id_column is not None and layer.id_column in row:
                feature["id"] = row[layer.id_column]
            yield separator + json.dumps(
                feature, ensure_ascii=False, default=str, allow_nan=False
            ).encode("utf-8")
            separator = b","
        yield b"]}"
