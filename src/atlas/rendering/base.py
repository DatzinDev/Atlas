"""Renderer contract and the affine mapping from world coordinates to pixels."""

import math
from collections.abc import Callable, Sequence
from typing import Literal, Protocol

from pydantic import BaseModel, ConfigDict
from shapely.geometry.base import BaseGeometry  # type: ignore[import-untyped]

from atlas.core.types import BBox
from atlas.styles.models import Style


class RenderLayer(BaseModel):
    model_config = ConfigDict(arbitrary_types_allowed=True)

    geometries: Sequence[BaseGeometry]
    style: Style


class MapRenderer(Protocol):
    def render(
        self,
        layers: Sequence[RenderLayer],
        *,
        bbox: BBox,
        width: int,
        height: int,
        image_format: Literal["png", "jpeg"] = "png",
        transparent: bool = False,
        background: str = "#FFFFFF",
    ) -> bytes: ...


def world_to_pixel(
    bbox: BBox, width: int, height: int
) -> Callable[[float, float], tuple[float, float]]:
    if (
        not all(math.isfinite(value) for value in bbox)
        or bbox[0] >= bbox[2]
        or bbox[1] >= bbox[3]
        or width <= 0
        or height <= 0
    ):
        raise ValueError("bbox must be finite and ordered; image dimensions must be positive")
    minx, miny, maxx, maxy = bbox
    scale_x, scale_y = width / (maxx - minx), height / (maxy - miny)
    return lambda x, y: ((x - minx) * scale_x, (maxy - y) * scale_y)
