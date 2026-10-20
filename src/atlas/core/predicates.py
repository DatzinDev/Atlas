"""Bounded attribute predicates, independent of transport and database types."""

from __future__ import annotations

import json
import math
from typing import Annotated, Any, Literal

from pydantic import BaseModel, ConfigDict, Field, TypeAdapter, ValidationError
from shapely.errors import GEOSException  # type: ignore[import-untyped]
from shapely.geometry import shape  # type: ignore[import-untyped]

from atlas.core.config import (
    FILTER_MAX_BYTES,
    FILTER_MAX_COORDINATES,
    FILTER_MAX_IN_VALUES,
    AtlasConfig,
)
from atlas.core.exceptions import InvalidFilter

Scalar = str | bool | int | float


class PredicateModel(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
        strict=True,
        frozen=True,
        allow_inf_nan=False,
        revalidate_instances="always",
    )


class Comparison(PredicateModel):
    op: Literal["eq", "ne", "gt", "gte", "lt", "lte", "like", "ilike"]
    field: str = Field(min_length=1)
    value: Scalar


class Membership(PredicateModel):
    op: Literal["in"]
    field: str = Field(min_length=1)
    value: list[Scalar] = Field(min_length=1)


class NullCheck(PredicateModel):
    op: Literal["is_null", "is_not_null"]
    field: str = Field(min_length=1)


class Group(PredicateModel):
    op: Literal["and", "or"]
    args: list[Predicate] = Field(min_length=1)


class Negation(PredicateModel):
    op: Literal["not"]
    arg: Predicate


class SpatialPredicate(PredicateModel):
    op: Literal["intersects", "within"]
    geometry: dict[str, Any]
    srid: int = Field(gt=0)


Predicate = Annotated[
    Comparison | Membership | NullCheck | Group | Negation | SpatialPredicate,
    Field(discriminator="op"),
]
_ADAPTER: TypeAdapter[Predicate] = TypeAdapter(Predicate)


def _model_dict(value: Any) -> dict[str, Any]:
    if not isinstance(value, PredicateModel):
        raise TypeError("expected a predicate model")
    return {key: getattr(value, key) for key in type(value).model_fields}


def _check_coordinates(geometry: Any, remaining: int) -> int:
    if not isinstance(geometry, dict) or set(geometry) != {"type", "coordinates"}:
        raise InvalidFilter("geometry requires type and coordinates only")
    kind, coordinates = geometry["type"], geometry["coordinates"]
    if kind not in ("Polygon", "MultiPolygon"):
        raise InvalidFilter("only Polygon/MultiPolygon selections are supported")
    polygons = [coordinates] if kind == "Polygon" else coordinates
    if not isinstance(polygons, list) or not polygons:
        raise InvalidFilter("polygon cannot be empty")
    count = 0
    for polygon in polygons:
        if not isinstance(polygon, list) or not polygon:
            raise InvalidFilter("polygon requires rings")
        for ring in polygon:
            if not isinstance(ring, list) or len(ring) < 4:
                raise InvalidFilter("ring requires at least four positions")
            for point in ring:
                count += 1
                if count > remaining:
                    raise InvalidFilter("predicate exceeds coordinate limit")
                if (
                    not isinstance(point, list)
                    or len(point) != 2
                    or any(type(v) not in (int, float) or not math.isfinite(v) for v in point)
                ):
                    raise InvalidFilter("positions must contain finite XY numbers")
            if ring[0] != ring[-1]:
                raise InvalidFilter("rings must be closed")
    return count


def validate_predicate(
    value: Any,
    *,
    max_depth: int,
    max_nodes: int,
    max_bytes: int = FILTER_MAX_BYTES,
    max_coordinates: int = FILTER_MAX_COORDINATES,
    max_in_values: int = FILTER_MAX_IN_VALUES,
) -> Predicate | None:
    """Check tree budgets iteratively before Pydantic's recursive validation."""
    if any(
        type(v) is not int or v < 1
        for v in (max_depth, max_nodes, max_bytes, max_coordinates, max_in_values)
    ):
        raise InvalidFilter("predicate limits must be positive integers")
    if value is None:
        return None
    stack = [(value, 1)]
    nodes = coordinates = 0
    geometries = []
    while stack:
        node, depth = stack.pop()
        nodes += 1
        if depth > max_depth or nodes > max_nodes:
            raise InvalidFilter("predicate exceeds depth or node limit")
        if isinstance(node, PredicateModel):
            node = _model_dict(node)
        if not isinstance(node, dict):
            raise InvalidFilter("predicate must be an object")
        if node.get("op") in ("and", "or"):
            children = node.get("args")
            if not isinstance(children, list) or not children:
                raise InvalidFilter("predicate group requires non-empty args")
            if nodes + len(stack) + len(children) > max_nodes:
                raise InvalidFilter("predicate exceeds node limit")
            stack.extend((child, depth + 1) for child in children)
        elif node.get("op") == "in":
            values = node.get("value")
            if not isinstance(values, list) or not 1 <= len(values) <= max_in_values:
                raise InvalidFilter("IN requires a non-empty bounded list")
        elif node.get("op") == "not":
            stack.append((node.get("arg"), depth + 1))
        elif node.get("op") in ("within", "intersects"):
            geometry = node.get("geometry")
            try:
                coordinates += _check_coordinates(geometry, max_coordinates - coordinates)
            except (ValueError, OverflowError) as exc:
                raise InvalidFilter("invalid polygon coordinates") from exc
            geometries.append(geometry)
    try:
        size = 0
        encoder = json.JSONEncoder(
            default=_model_dict, allow_nan=False, ensure_ascii=False, separators=(",", ":")
        )
        for chunk in encoder.iterencode(value):
            size += len(chunk.encode("utf-8"))
            if size > max_bytes:
                raise InvalidFilter("predicate exceeds byte limit")
        result = _ADAPTER.validate_python(value)
        for geometry in geometries:
            native = shape(geometry)
            if native.is_empty or not native.is_valid:
                raise InvalidFilter("polygon must be non-empty and topologically valid")
        return result
    except (
        ValidationError,
        RecursionError,
        TypeError,
        ValueError,
        OverflowError,
        GEOSException,
    ) as exc:
        raise InvalidFilter("invalid predicate") from exc


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise InvalidFilter("duplicate predicate key")
        result[key] = value
    return result


def _reject_constant(value: str) -> Any:
    raise InvalidFilter("non-finite JSON number")


def parse_predicate(value: str | None, config: AtlasConfig) -> Predicate | None:
    """Decode bounded JSON, then validate one predicate with the configured budgets."""
    if value is None:
        return None
    try:
        if len(value.encode("utf-8")) > config.filter_max_bytes:
            raise InvalidFilter("predicate exceeds byte limit")
        decoded = json.loads(
            value, object_pairs_hook=_unique_object, parse_constant=_reject_constant
        )
    except (ValueError, RecursionError) as exc:
        raise InvalidFilter("invalid predicate JSON") from exc
    return validate_predicate(
        decoded,
        max_depth=config.filter_max_depth,
        max_nodes=config.filter_max_nodes,
        max_bytes=config.filter_max_bytes,
        max_coordinates=config.filter_max_coordinates,
        max_in_values=config.filter_max_in_values,
    )
