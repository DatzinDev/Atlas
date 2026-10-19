"""The renderer boundary accepts external implementations and inverts only pixel Y."""

import ast
import inspect

import pytest
from shapely.geometry import Point

from atlas.rendering import base
from atlas.rendering.base import MapRenderer, RenderLayer, world_to_pixel
from atlas.styles.models import PointStyle


@pytest.mark.parametrize(
    "world,pixel",
    [
        ((10, 40), (0, 0)),
        ((30, 40), (300, 0)),
        ((10, 20), (0, 100)),
        ((30, 20), (300, 100)),
        ((20, 30), (150, 50)),
    ],
)
def test_mapping_with_independent_scales(world, pixel):
    assert world_to_pixel((10, 20, 30, 40), 300, 100)(*world) == pixel


@pytest.mark.parametrize(
    "bbox,width,height",
    [
        ((0, 0, 0, 1), 10, 10),
        ((1, 0, 0, 1), 10, 10),
        ((0, 1, 1, 0), 10, 10),
        ((0, 0, float("nan"), 1), 10, 10),
        ((0, 0, 1, float("inf")), 10, 10),
        ((0, 0, 1, 1), 0, 10),
        ((0, 0, 1, 1), 10, -1),
    ],
)
def test_invalid_mapping(bbox, width, height):
    with pytest.raises(ValueError):
        world_to_pixel(bbox, width, height)


def test_external_renderer_without_inheritance():
    class ExternalRenderer:
        def render(self, layers, **kwargs):
            return b"external"

    renderer: MapRenderer = ExternalRenderer()
    layer = RenderLayer(geometries=[Point(1, 2)], style=PointStyle())
    assert renderer.render([layer], bbox=(0, 0, 2, 3), width=10, height=10) == b"external"
    assert MapRenderer._is_protocol


def test_no_database_or_layer_imports():
    tree = ast.parse(inspect.getsource(base))
    for node in ast.walk(tree):
        modules = []
        if isinstance(node, ast.ImportFrom):
            modules = [node.module or ""]
        elif isinstance(node, ast.Import):
            modules = [alias.name for alias in node.names]
        assert all(not module.startswith(("atlas.postgis", "atlas.layers")) for module in modules)
