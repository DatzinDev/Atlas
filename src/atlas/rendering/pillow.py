"""Stateless Pillow rendering of projected Shapely geometries."""

from collections.abc import Callable, Sequence
from io import BytesIO
from typing import Any, Literal

from PIL import Image, ImageDraw
from shapely.geometry.base import BaseGeometry  # type: ignore[import-untyped]

from atlas.core.types import BBox
from atlas.rendering.base import RenderLayer, world_to_pixel
from atlas.styles.models import LineStyle, PointStyle, PolygonStyle, Style, rgba


def _draw(
    image: Image.Image,
    geometry: BaseGeometry,
    style: Style,
    project: Callable[[float, float], tuple[float, float]],
    scale: int,
) -> None:
    if geometry.is_empty or not geometry.is_valid:
        return
    kind = geometry.geom_type
    if kind.startswith("Multi") or kind == "GeometryCollection":
        for part in geometry.geoms:
            _draw(image, part, style, project, scale)
        return

    def points(coords: Any) -> list[tuple[float, float]]:
        return [project(coord[0], coord[1]) for coord in coords]

    overlay = Image.new("RGBA", image.size)
    draw = ImageDraw.Draw(overlay)
    if kind == "Polygon":
        polygon = style if isinstance(style, PolygonStyle) else PolygonStyle()
        mask = Image.new("L", image.size)
        brush = ImageDraw.Draw(mask)
        brush.polygon(points(geometry.exterior.coords), fill=round(polygon.fill_opacity * 255))
        for ring in geometry.interiors:
            brush.polygon(points(ring.coords), fill=0)
        fill = Image.new("RGBA", image.size, rgba(polygon.fill, 1))
        fill.putalpha(mask)
        image.alpha_composite(fill)
        if polygon.stroke_width > 0:
            for ring in (geometry.exterior, *geometry.interiors):
                draw.line(
                    points(ring.coords),
                    fill=rgba(polygon.stroke, polygon.stroke_opacity),
                    width=max(1, round(polygon.stroke_width * scale)),
                    joint="curve",
                )
    elif kind in ("LineString", "LinearRing"):
        line = style if isinstance(style, (LineStyle, PolygonStyle)) else LineStyle()
        if line.stroke_width > 0:
            draw.line(
                points(geometry.coords),
                fill=rgba(line.stroke, line.stroke_opacity),
                width=max(1, round(line.stroke_width * scale)),
                joint="curve",
            )
    elif kind == "Point":
        point = style if isinstance(style, PointStyle) else PointStyle()
        x, y = project(geometry.x, geometry.y)
        radius = point.radius * scale
        bounds = (x - radius, y - radius, x + radius, y + radius)
        draw.ellipse(bounds, fill=rgba(point.fill, point.fill_opacity))
        image.alpha_composite(overlay)
        overlay = Image.new("RGBA", image.size)
        if point.stroke_width > 0:
            ImageDraw.Draw(overlay).ellipse(
                bounds,
                outline=rgba(point.stroke, 1),
                width=max(1, round(point.stroke_width * scale)),
            )
    image.alpha_composite(overlay)


class PillowRenderer:
    def __init__(self, supersample: int = 2) -> None:
        if not isinstance(supersample, int) or supersample < 1:
            raise ValueError("supersample must be a positive integer")
        self.supersample = supersample

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
    ) -> bytes:
        if image_format not in ("png", "jpeg"):
            raise ValueError("image_format must be png or jpeg")
        scale = self.supersample
        project = world_to_pixel(bbox, width * scale, height * scale)
        alpha = 0 if transparent and image_format == "png" else 1
        image = Image.new("RGBA", (width * scale, height * scale), rgba(background, alpha))
        for layer in layers:
            for geometry in layer.geometries:
                _draw(image, geometry, layer.style, project, scale)
        if scale != 1:
            image = image.resize((width, height), Image.Resampling.LANCZOS)
        if image_format == "jpeg":
            image = image.convert("RGB")
        output = BytesIO()
        image.save(output, format=image_format.upper())
        return output.getvalue()
