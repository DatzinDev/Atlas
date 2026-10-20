"""Validated XYZ coordinates and affine Web Mercator tile bounds."""

from pydantic import BaseModel, ConfigDict, Field, model_validator

from atlas.core.exceptions import InvalidParameter
from atlas.core.types import BBox

WEB_MERCATOR_HALF_WORLD = 20037508.342789244


class TileCoord(BaseModel):
    model_config = ConfigDict(strict=True, frozen=True, extra="forbid")
    z: int = Field(ge=0, le=30)
    x: int = Field(ge=0)
    y: int = Field(ge=0)

    @model_validator(mode="after")
    def within_grid(self) -> "TileCoord":
        if self.x >= 2**self.z or self.y >= 2**self.z:
            raise InvalidParameter("tile coordinates lie outside the XYZ grid")
        return self


def tile_bounds(tile: TileCoord, *, max_zoom: int = 22) -> BBox:
    if type(max_zoom) is not int or not 0 <= max_zoom <= 30 or tile.z > max_zoom:
        raise InvalidParameter("tile zoom exceeds max_tile_zoom")
    step = 2 * WEB_MERCATOR_HALF_WORLD / 2**tile.z
    return (
        -WEB_MERCATOR_HALF_WORLD + tile.x * step,
        WEB_MERCATOR_HALF_WORLD - (tile.y + 1) * step,
        -WEB_MERCATOR_HALF_WORLD + (tile.x + 1) * step,
        WEB_MERCATOR_HALF_WORLD - tile.y * step,
    )
