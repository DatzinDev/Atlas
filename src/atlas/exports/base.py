"""Format registry and the streaming exporter contract; optional drivers stay optional."""

from __future__ import annotations

import re
from collections.abc import AsyncIterator
from importlib.util import find_spec
from typing import TYPE_CHECKING, Any, ClassVar, Protocol

from atlas.core.exceptions import UnsupportedFormat

if TYPE_CHECKING:
    from atlas.layers.models import Layer


class Exporter(Protocol):
    format: ClassVar[str]
    media_type: ClassVar[str]
    extension: ClassVar[str]
    streaming: ClassVar[bool]

    def export(self, rows: AsyncIterator[dict[str, Any]], layer: Layer) -> AsyncIterator[bytes]: ...


EXPORTERS: dict[str, Exporter] = {}


def available_formats() -> list[str]:
    return sorted(
        name for name in EXPORTERS if name not in ("gpkg", "shp") or find_spec("fiona") is not None
    )


def get_exporter(format: str) -> Exporter:
    if format not in available_formats():
        raise UnsupportedFormat("unsupported download format", formats=available_formats())
    return EXPORTERS[format]


def download_filename(layer: Layer, exporter: Exporter) -> str:
    name = re.sub(r"[^a-zA-Z0-9_-]+", "_", layer.name).strip("_-") or "layer"
    return f"{name}.{exporter.extension}"
