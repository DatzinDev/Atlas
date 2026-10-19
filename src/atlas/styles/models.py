"""Validated style data; no renderer or database dependency."""

import re
from typing import Annotated

from pydantic import BaseModel, ConfigDict, Field, StringConstraints

from atlas.core.exceptions import InvalidGeometry
from atlas.core.types import GeometryType

Color = Annotated[str, StringConstraints(pattern=r"^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$")]
Opacity = Annotated[float, Field(ge=0, le=1)]
Size = Annotated[float, Field(ge=0, allow_inf_nan=False)]


class PolygonStyle(BaseModel):
    model_config = ConfigDict(extra="forbid")
    fill: Color = "#3178c6"
    fill_opacity: Opacity = 0.7
    stroke: Color = "#ffffff"
    stroke_width: Size = 1.0
    stroke_opacity: Opacity = 1.0


class LineStyle(BaseModel):
    model_config = ConfigDict(extra="forbid")
    stroke: Color = "#3178c6"
    stroke_width: Size = 2.0
    stroke_opacity: Opacity = 1.0


class PointStyle(BaseModel):
    model_config = ConfigDict(extra="forbid")
    radius: Size = 5.0
    fill: Color = "#3178c6"
    fill_opacity: Opacity = 1.0
    stroke: Color = "#ffffff"
    stroke_width: Size = 1.0


Style = PolygonStyle | LineStyle | PointStyle


def default_style(geometry_type: GeometryType) -> Style:
    if geometry_type in ("Polygon", "MultiPolygon"):
        return PolygonStyle()
    if geometry_type in ("LineString", "MultiLineString"):
        return LineStyle()
    if geometry_type in ("Point", "MultiPoint"):
        return PointStyle()
    raise InvalidGeometry("no default style exists for this geometry type")


def parse_color(value: str) -> tuple[int, int, int]:
    if re.fullmatch(r"#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})", value) is None:
        raise ValueError("color must contain three or six hexadecimal digits")
    digits = value.removeprefix("#")
    if len(digits) == 3:
        digits = "".join(char * 2 for char in digits)
    return int(digits[:2], 16), int(digits[2:4], 16), int(digits[4:], 16)


def rgba(color: str, opacity: float) -> tuple[int, int, int, int]:
    if not 0 <= opacity <= 1:
        raise ValueError("opacity must be between zero and one")
    return *parse_color(color), round(opacity * 255)
