"""Pixel samples exercise shapes, holes, compositing, antialiasing and image formats."""

from concurrent.futures import ThreadPoolExecutor
from io import BytesIO

import pytest
from PIL import Image
from shapely.geometry import (
    GeometryCollection,
    LineString,
    MultiLineString,
    MultiPoint,
    MultiPolygon,
    Point,
    Polygon,
    box,
)

from atlas.rendering.base import MapRenderer, RenderLayer
from atlas.rendering.pillow import PillowRenderer
from atlas.styles.models import LineStyle, PointStyle, PolygonStyle


def render(geometries, style, **kwargs):
    data = PillowRenderer().render(
        [RenderLayer(geometries=geometries, style=style)],
        bbox=(0, 0, 10, 10),
        width=100,
        height=100,
        **kwargs,
    )
    return Image.open(BytesIO(data))


def test_hole_preserves_lower_layer_and_reference_pixels():
    hole = Polygon([(1, 1), (9, 1), (9, 9), (1, 9)], holes=[[(3, 3), (7, 3), (7, 7), (3, 7)]])
    renderer: MapRenderer = PillowRenderer()
    layers = [
        RenderLayer(
            geometries=[box(0, 0, 10, 10)],
            style=PolygonStyle(fill="#0000ff", fill_opacity=1, stroke_width=0),
        ),
        RenderLayer(
            geometries=[hole], style=PolygonStyle(fill="#ff0000", fill_opacity=1, stroke_width=0)
        ),
    ]
    result = Image.open(
        BytesIO(renderer.render(layers, bbox=(0, 0, 10, 10), width=100, height=100))
    )
    reference = {(5, 50): (0, 0, 255, 255), (20, 50): (255, 0, 0, 255), (50, 50): (0, 0, 255, 255)}
    for pixel, color in reference.items():
        assert result.getpixel(pixel) == color
    layers.reverse()
    result = Image.open(
        BytesIO(renderer.render(layers, bbox=(0, 0, 10, 10), width=100, height=100))
    )
    assert result.getpixel((20, 50)) == (0, 0, 255, 255)


@pytest.mark.parametrize(
    "geometry,style,pixel",
    [
        (
            MultiPolygon([box(1, 1, 3, 3), box(7, 7, 9, 9)]),
            PolygonStyle(fill="#ff0000", fill_opacity=1),
            (20, 80),
        ),
        (LineString([(1, 5), (9, 5)]), LineStyle(stroke="#ff0000", stroke_width=5), (50, 50)),
        (
            MultiLineString([[(1, 2), (9, 2)], [(1, 8), (9, 8)]]),
            LineStyle(stroke="#ff0000", stroke_width=5),
            (50, 20),
        ),
        (Point(5, 5), PointStyle(fill="#ff0000", radius=8, stroke_width=0), (50, 50)),
        (
            MultiPoint([(2, 2), (8, 8)]),
            PointStyle(fill="#ff0000", radius=8, stroke_width=0),
            (20, 80),
        ),
    ],
)
def test_primitive_and_multi_geometries(geometry, style, pixel):
    image = render([geometry], style, transparent=True)
    assert image.getpixel(pixel) == pytest.approx((255, 0, 0, 255), abs=3)
    assert image.getpixel((0, 0))[3] == 0


def test_collection_empty_and_invalid_geometries():
    invalid = Polygon([(1, 1), (9, 9), (1, 9), (9, 1), (1, 1)])
    image = render(
        [Point(), invalid, GeometryCollection([box(1, 1, 3, 3), Point(8, 8)])],
        PolygonStyle(fill="#ff0000", fill_opacity=1),
        transparent=True,
    )
    assert image.getpixel((20, 80)) == (255, 0, 0, 255)
    assert image.getpixel((80, 20))[3] == 255
    assert image.getpixel((50, 50))[3] == 0


@pytest.mark.parametrize(
    "geometry,style",
    [
        (box(1, 1, 9, 9), PolygonStyle(fill="#ff0000", fill_opacity=0.5, stroke_width=0)),
        (
            LineString([(1, 5), (9, 5)]),
            LineStyle(stroke="#ff0000", stroke_width=5, stroke_opacity=0.5),
        ),
        (Point(5, 5), PointStyle(fill="#ff0000", fill_opacity=0.5, radius=10, stroke_width=0)),
    ],
)
def test_opacity(geometry, style):
    assert render([geometry], style, transparent=True).getpixel((50, 50)) == pytest.approx(
        (255, 0, 0, 128), abs=3
    )


def test_polygon_and_point_strokes():
    image = render(
        [box(2, 2, 8, 8)], PolygonStyle(fill_opacity=0, stroke="#00ff00", stroke_width=6)
    )
    assert image.getpixel((20, 50))[:3] == (0, 255, 0)
    image = render([Point(5, 5)], PointStyle(radius=10, stroke="#00ff00", stroke_width=5))
    assert image.getpixel((57, 50))[:3] == pytest.approx((0, 255, 0), abs=6)


def test_supersampling_and_jpeg_background():
    image = render(
        [Polygon([(1, 1), (9, 1), (5, 9)])],
        PolygonStyle(fill="#ff0000", fill_opacity=1, stroke_width=0),
        transparent=True,
    )
    assert image.size == (100, 100)
    assert sum(image.getchannel("A").histogram()[1:255]) > 0
    image = render([], PointStyle(), image_format="jpeg", transparent=True, background="#00ff00")
    assert image.format == "JPEG" and image.mode == "RGB"
    assert image.getpixel((50, 50)) == pytest.approx((0, 255, 0), abs=2)


def test_reentrant_renderer_and_invalid_supersample():
    renderer = PillowRenderer(supersample=1)

    def image(color):
        return Image.open(
            BytesIO(renderer.render([], bbox=(0, 0, 1, 1), width=10, height=10, background=color))
        ).getpixel((5, 5))

    with ThreadPoolExecutor(2) as pool:
        assert list(pool.map(image, ["#ff0000", "#0000ff"])) == [(255, 0, 0, 255), (0, 0, 255, 255)]
    for value in (0, -1, 1.5):
        with pytest.raises(ValueError):
            PillowRenderer(supersample=value)
