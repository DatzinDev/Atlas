"""Style defaults, color parsing and validation at construction."""

import pytest
from pydantic import ValidationError

from atlas.core.exceptions import InvalidGeometry
from atlas.styles.models import (
    LineStyle,
    PointStyle,
    PolygonStyle,
    default_style,
    parse_color,
    rgba,
)


@pytest.mark.parametrize("value", ["#abc", "abc", "#aabbcc", "aabbcc", "#AABBCC"])
def test_hex_colors(value: str) -> None:
    assert parse_color(value) == (170, 187, 204)
    assert PolygonStyle(fill=value).fill == value


@pytest.mark.parametrize("value", ["red", "#12", "#gggggg", "aabbcc\n", "", "#abcd"])
def test_invalid_colors(value: str) -> None:
    with pytest.raises(ValueError):
        parse_color(value)
    with pytest.raises(ValidationError):
        PolygonStyle(fill=value)


@pytest.mark.parametrize("opacity", [-0.1, 1.1, float("nan"), float("inf")])
def test_invalid_opacity(opacity: float) -> None:
    with pytest.raises(ValidationError):
        PolygonStyle(fill_opacity=opacity)
    with pytest.raises(ValueError):
        rgba("abc", opacity)


@pytest.mark.parametrize(
    "geometry,style",
    [
        ("Polygon", PolygonStyle),
        ("MultiPolygon", PolygonStyle),
        ("LineString", LineStyle),
        ("MultiLineString", LineStyle),
        ("Point", PointStyle),
        ("MultiPoint", PointStyle),
    ],
)
def test_defaults(geometry, style) -> None:
    assert default_style(geometry) == style()


def test_default_values_and_alpha() -> None:
    assert PolygonStyle().model_dump() == dict(
        fill="#3178c6", fill_opacity=0.7, stroke="#ffffff", stroke_width=1.0, stroke_opacity=1.0
    )
    assert LineStyle().model_dump() == dict(stroke="#3178c6", stroke_width=2.0, stroke_opacity=1.0)
    assert PointStyle().model_dump() == dict(
        radius=5.0, fill="#3178c6", fill_opacity=1.0, stroke="#ffffff", stroke_width=1.0
    )
    assert rgba("abc", 0.5) == (170, 187, 204, 128)
    with pytest.raises(ValidationError):
        PointStyle(radius=-1)
    with pytest.raises(InvalidGeometry):
        default_style("GeometryCollection")
