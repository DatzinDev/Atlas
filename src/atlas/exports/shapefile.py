"""Write a bounded Shapefile ZIP with explicit field mapping and temporary cleanup."""

from __future__ import annotations

import json
import math
import re
import unicodedata
from collections.abc import AsyncIterator
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import TYPE_CHECKING, Any, ClassVar
from zipfile import ZIP_DEFLATED, ZipFile

from atlas.core.exceptions import InvalidGeometry, InvalidParameter, QueryTooLarge
from atlas.exports.base import download_filename

if TYPE_CHECKING:
    from atlas.layers.models import Layer

MAX_BYTES = 2 * 1024**3
TYPES = {
    "int2": "int:11",
    "int4": "int:11",
    "bool": "int:1",
    "float4": "float:32.15",
    "float8": "float:32.15",
    "date": "date",
}
GEOMETRIES = {
    "Point": "Point",
    "MultiPoint": "MultiPoint",
    "LineString": "LineString",
    "MultiLineString": "LineString",
    "Polygon": "Polygon",
    "MultiPolygon": "Polygon",
}


def _field_names(names: list[str]) -> dict[str, str]:
    mapping = {}
    used = set()
    for name in names:
        stem = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode()
        stem = re.sub(r"[^a-zA-Z0-9_]", "_", stem) or "field"
        candidate, index = stem[:10], 1
        while candidate.lower() in used:
            suffix = f"_{index}"
            candidate, index = stem[: 10 - len(suffix)] + suffix, index + 1
        used.add(candidate.lower())
        mapping[name] = candidate
    return mapping


def _check_size(directory: Path) -> None:
    if sum(path.stat().st_size for path in directory.iterdir() if path.is_file()) > MAX_BYTES:
        raise QueryTooLarge("Shapefile exceeds the 2 GB dataset limit", maximum=MAX_BYTES)


class ShapefileExporter:
    format: ClassVar[str] = "shp"
    media_type: ClassVar[str] = "application/zip"
    extension: ClassVar[str] = "zip"
    streaming: ClassVar[bool] = False

    async def export(
        self, rows: AsyncIterator[dict[str, Any]], layer: Layer
    ) -> AsyncIterator[bytes]:
        import fiona  # type: ignore[import-untyped]

        if len(layer.attributes) > 255:
            raise QueryTooLarge("Shapefile supports at most 255 fields", maximum=255)
        if layer.geometry_type not in GEOMETRIES:
            raise InvalidGeometry("Shapefile requires a single supported geometry family")
        geometry_type = GEOMETRIES[layer.geometry_type]
        names = _field_names(layer.attribute_names)
        fields = {a.name: TYPES.get(a.data_type, "str:254") for a in layer.attributes}
        schema = {"geometry": geometry_type, "properties": {names[k]: v for k, v in fields.items()}}
        with TemporaryDirectory(prefix="atlas-shp-") as directory:
            root = Path(directory)
            try:
                with fiona.open(
                    root / "data.shp",
                    "w",
                    driver="ESRI Shapefile",
                    schema=schema,
                    crs=f"EPSG:{layer.srid}",
                    encoding="UTF-8",
                    **{"2GB_LIMIT": "YES"},
                ) as target:
                    count = 0
                    async for row in rows:
                        geometry = row.get(layer.geometry_column)
                        if (
                            geometry is not None
                            and GEOMETRIES.get(geometry["type"]) != geometry_type
                        ):
                            raise InvalidGeometry(
                                "mixed geometry families cannot be written to Shapefile"
                            )
                        values = {}
                        for name, kind in fields.items():
                            value = row.get(name)
                            if value is not None:
                                if kind.startswith("str"):
                                    if isinstance(value, (dict, list)):
                                        value = json.dumps(value, ensure_ascii=False, default=str)
                                    elif isinstance(value, (bytes, memoryview)):
                                        value = bytes(value).hex()
                                    else:
                                        value = (
                                            value.isoformat()
                                            if hasattr(value, "isoformat")
                                            else str(value)
                                        )
                                    if len(value.encode("utf-8")) > 254:
                                        raise InvalidParameter(
                                            "Shapefile text exceeds 254 UTF-8 bytes", field=name
                                        )
                                elif kind.startswith("float"):
                                    if not math.isfinite(value) or len(f"{value:.15f}") > 32:
                                        raise InvalidParameter(
                                            "value exceeds Shapefile numeric range", field=name
                                        )
                                elif kind == "date":
                                    value = (
                                        value.isoformat() if hasattr(value, "isoformat") else value
                                    )
                                elif kind == "int:1":
                                    value = int(value)
                            values[names[name]] = value
                        target.write({"geometry": geometry, "properties": values})
                        count += 1
                        if count % 32 == 0:
                            target.flush()
                            _check_size(root)
            except (RuntimeError, fiona.errors.FionaError) as exc:
                if "2gb" in str(exc).lower() or "2 gb" in str(exc).lower():
                    raise QueryTooLarge(
                        "Shapefile exceeds the 2 GB limit", maximum=MAX_BYTES
                    ) from exc
                raise
            (root / "data.cpg").write_text("UTF-8", encoding="ascii")
            mapping = "\n".join(f"{original} -> {short}" for original, short in names.items())
            converted = ", ".join(name for name, kind in fields.items() if kind.startswith("str"))
            readme = (
                "Exportación Shapefile de Atlas.\n"
                "Límites: 255 campos; 2 GB; nombres ASCII de 10 caracteres.\n"
                "Texto: máximo 254 bytes UTF-8; números reales: 15 decimales.\n"
                "int8, numeric, datetime, arrays y otros tipos no portables "
                "se guardan como texto.\n"
                f"Campos de texto: {converted}\n\nCorrespondencia de campos:\n{mapping}\n"
            )
            (root / "README.txt").write_text(readme, encoding="utf-8")
            _check_size(root)
            archive = root / "data.zip"
            stem = Path(download_filename(layer, self)).stem
            with ZipFile(archive, "w", ZIP_DEFLATED) as bundle:
                for suffix in ("shp", "shx", "dbf", "prj", "cpg"):
                    bundle.write(root / f"data.{suffix}", f"{stem}.{suffix}")
                bundle.write(root / "README.txt", "README.txt")
            if archive.stat().st_size > MAX_BYTES:
                raise QueryTooLarge("Shapefile ZIP exceeds the 2 GB limit", maximum=MAX_BYTES)
            with archive.open("rb") as source:
                while chunk := source.read(65536):
                    yield chunk
