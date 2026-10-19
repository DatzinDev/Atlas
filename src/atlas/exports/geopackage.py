"""Build a single-layer GeoPackage on disk, then stream and always remove it."""

from __future__ import annotations

import json
from collections.abc import AsyncIterator
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import TYPE_CHECKING, Any, ClassVar

if TYPE_CHECKING:
    from atlas.layers.models import Layer

TYPES = {
    "int2": "int32",
    "int4": "int32",
    "int8": "int64",
    "float4": "float",
    "float8": "float",
    "bool": "bool",
    "date": "date",
    "timestamp": "datetime",
    "timestamptz": "datetime",
    "bytea": "bytes",
}


class GeoPackageExporter:
    format: ClassVar[str] = "gpkg"
    media_type: ClassVar[str] = "application/geopackage+sqlite3"
    extension: ClassVar[str] = "gpkg"
    streaming: ClassVar[bool] = False

    async def export(
        self, rows: AsyncIterator[dict[str, Any]], layer: Layer
    ) -> AsyncIterator[bytes]:
        import fiona  # type: ignore[import-untyped]

        fields = {a.name: TYPES.get(a.data_type, "str") for a in layer.attributes}
        fid = "_atlas_fid"
        while fid.lower() in {name.lower() for name in (*fields, layer.geometry_column)}:
            fid += "_"
        schema = {
            "geometry": "Unknown" if layer.geometry_type == "Geometry" else layer.geometry_type,
            "properties": fields,
        }
        with TemporaryDirectory(prefix="atlas-gpkg-") as directory:
            path = Path(directory) / "data.gpkg"
            with fiona.open(
                path,
                "w",
                driver="GPKG",
                schema=schema,
                crs=f"EPSG:{layer.srid}",
                layer=layer.name,
                FID=fid,
                GEOMETRY_NAME=layer.geometry_column,
            ) as target:
                async for row in rows:
                    values = {}
                    for name, kind in fields.items():
                        value = row.get(name)
                        if value is not None:
                            if kind == "str":
                                value = (
                                    json.dumps(value, ensure_ascii=False, default=str)
                                    if isinstance(value, (dict, list))
                                    else str(value)
                                )
                            elif kind in ("date", "datetime"):
                                value = value.isoformat() if hasattr(value, "isoformat") else value
                            elif kind == "bytes":
                                value = bytes(value)
                        values[name] = value
                    target.write({"geometry": row.get(layer.geometry_column), "properties": values})
            with path.open("rb") as source:
                while chunk := source.read(65536):
                    yield chunk
