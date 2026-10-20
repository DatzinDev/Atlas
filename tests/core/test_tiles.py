"""XYZ reference bounds, shared edges and strict coordinate validation."""

import pytest
from pydantic import ValidationError

from atlas.core.exceptions import InvalidParameter
from atlas.core.tiles import WEB_MERCATOR_HALF_WORLD as H
from atlas.core.tiles import TileCoord, tile_bounds


def test_reference_world_children_and_shared_edges():
    assert tile_bounds(TileCoord(z=0, x=0, y=0)) == (-H, -H, H, H)
    assert tile_bounds(TileCoord(z=1, x=0, y=0)) == (-H, 0, 0, H)
    assert tile_bounds(TileCoord(z=1, x=1, y=0)) == (0, 0, H, H)
    assert tile_bounds(TileCoord(z=1, x=0, y=1)) == (-H, -H, 0, 0)
    assert tile_bounds(TileCoord(z=1, x=1, y=1)) == (0, -H, H, 0)
    assert tile_bounds(TileCoord(z=22, x=1, y=2))[2] == tile_bounds(TileCoord(z=22, x=2, y=2))[0]


@pytest.mark.parametrize(
    "z,x,y", [(-1, 0, 0), (1, -1, 0), (1, 0, -1), (1, 2, 0), (1, 0, 2), (True, 0, 0), (31, 0, 0)]
)
def test_invalid_coordinates(z, x, y):
    with pytest.raises((InvalidParameter, ValidationError)):
        TileCoord(z=z, x=x, y=y)


def test_configured_zoom():
    with pytest.raises(InvalidParameter):
        tile_bounds(TileCoord(z=23, x=0, y=0))
    assert tile_bounds(TileCoord(z=23, x=0, y=0), max_zoom=23)
