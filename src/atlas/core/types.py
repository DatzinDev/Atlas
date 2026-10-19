"""Geometry names and bounding boxes shared by metadata and styles."""

from typing import Literal

BBox = tuple[float, float, float, float]
GeometryType = Literal[
    "Point",
    "LineString",
    "Polygon",
    "MultiPoint",
    "MultiLineString",
    "MultiPolygon",
    "GeometryCollection",
    "Geometry",
]
