"""UTF-8 CSV with an Excel-friendly BOM and per-row geometry conversion."""

from __future__ import annotations

import codecs
import csv
from collections.abc import AsyncIterator
from io import StringIO
from typing import TYPE_CHECKING, Any, ClassVar, Literal

from shapely.geometry import shape  # type: ignore[import-untyped]

if TYPE_CHECKING:
    from atlas.layers.models import Layer


class CSVExporter:
    format: ClassVar[str] = "csv"
    media_type: ClassVar[str] = "text/csv"
    extension: ClassVar[str] = "csv"
    streaming: ClassVar[bool] = True

    def __init__(self, geometry: Literal["wkt", "centroid", "none"] = "wkt") -> None:
        if geometry not in ("wkt", "centroid", "none"):
            raise ValueError("CSV geometry must be wkt, centroid or none")
        self.geometry = geometry

    async def export(
        self, rows: AsyncIterator[dict[str, Any]], layer: Layer
    ) -> AsyncIterator[bytes]:
        names = layer.attribute_names
        if layer.id_column is not None and layer.id_column not in names:
            names.append(layer.id_column)
        extra = (
            ["geometry"]
            if self.geometry == "wkt"
            else ["lon", "lat"]
            if self.geometry == "centroid"
            else []
        )
        buffer = StringIO(newline="")
        writer = csv.writer(buffer)
        writer.writerow([*names, *extra])
        yield codecs.BOM_UTF8 + buffer.getvalue().encode("utf-8")
        async for row in rows:
            buffer.seek(0)
            buffer.truncate(0)
            values = [row.get(name) for name in names]
            geometry = row.get(layer.geometry_column)
            if self.geometry != "none":
                geometry = shape(geometry) if geometry is not None else None
                if self.geometry == "wkt":
                    values.append(geometry.wkt if geometry is not None else None)
                elif geometry is not None and not geometry.is_empty:
                    center = geometry.centroid
                    values.extend([center.x, center.y])
                else:
                    values.extend([None, None])
            writer.writerow(values)
            yield buffer.getvalue().encode("utf-8")
