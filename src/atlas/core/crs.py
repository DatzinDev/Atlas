"""Supported coordinate systems and their protocol axis order."""

import re
from typing import Literal

from pydantic import BaseModel, ConfigDict

from atlas.core.exceptions import UnsupportedCRS
from atlas.core.types import BBox


class CRSInfo(BaseModel):
    model_config = ConfigDict(frozen=True)
    srid: int
    code: str
    axis_order: Literal["xy", "yx"]
    bounds: BBox
    units: str


SUPPORTED_CRS: dict[int, CRSInfo] = {
    4326: CRSInfo(
        srid=4326, code="EPSG:4326", axis_order="yx", bounds=(-180, -90, 180, 90), units="degrees"
    ),
    3857: CRSInfo(
        srid=3857,
        code="EPSG:3857",
        axis_order="xy",
        bounds=(-20037508.342789244,) * 2 + (20037508.342789244,) * 2,
        units="metres",
    ),
}


def parse_crs(value: str) -> int:
    text = value.strip().upper()
    if text == "CRS:84":
        return require_crs(4326).srid
    match = re.fullmatch(r"(?:EPSG:|URN:OGC:DEF:CRS:EPSG::)?([0-9]+)", text)
    if match is None:
        raise UnsupportedCRS("unsupported CRS", supported=list(SUPPORTED_CRS))
    return require_crs(int(match[1])).srid


def require_crs(srid: int | str) -> CRSInfo:
    """Accept a textual CRS to retain CRS:84's xy axes without changing EPSG:4326."""
    alias = isinstance(srid, str) and srid.strip().upper() == "CRS:84"
    code = parse_crs(srid) if isinstance(srid, str) else srid
    if code not in SUPPORTED_CRS:
        raise UnsupportedCRS("unsupported CRS", supported=list(SUPPORTED_CRS))
    info = SUPPORTED_CRS[code]
    return info.model_copy(update={"code": "CRS:84", "axis_order": "xy"}) if alias else info
