"""Parse whitelisted attribute filters before their values reach PostgreSQL."""

from collections.abc import Mapping
from decimal import InvalidOperation
from typing import Any

from psycopg import sql
from pydantic import BaseModel

from atlas.core.exceptions import InvalidFilter
from atlas.core.predicates import Comparison, Group, Membership, Predicate, Scalar
from atlas.core.types import BBox
from atlas.layers.models import Layer
from atlas.postgis.predicates import compile_predicate
from atlas.postgis.queries import FILTER_OPERATORS, bbox_predicate
from atlas.postgis.values import cast_attribute_value

OPERATORS = FILTER_OPERATORS
RESERVED_PARAMS = frozenset(
    {
        "service",
        "version",
        "request",
        "typenames",
        "typename",
        "bbox",
        "srsname",
        "outputformat",
        "count",
        "startindex",
        "limit",
        "offset",
        "propertyname",
        "geometry",
        "format",
        "resulttype",
    }
)


class Filter(BaseModel):
    column: str
    operator: str
    value: Any


def _validated_filter(column: str, operator: str, value: Any, layer: Layer) -> Filter:
    attributes = {a.name: a for a in layer.attributes if a.name != layer.geometry_column}
    if column not in attributes:
        raise InvalidFilter("unknown attribute", column=column)
    if operator not in OPERATORS:
        raise InvalidFilter("unknown filter operator", operator=operator)
    if operator == "like" and attributes[column].data_type not in (
        "text",
        "varchar",
        "bpchar",
        "name",
    ):
        raise InvalidFilter("LIKE requires a text attribute", column=column)
    try:
        if operator == "in":
            values = value if isinstance(value, list) else str(value).split(",")
            if not values or any(str(v).strip() == "" for v in values):
                raise ValueError("empty IN member")
            value = [
                cast_attribute_value(
                    v.strip() if isinstance(v, str) else v, attributes[column].data_type
                )
                for v in values
            ]
        else:
            value = cast_attribute_value(value, attributes[column].data_type)
    except (ValueError, InvalidOperation, OverflowError) as exc:
        raise InvalidFilter("invalid attribute value", column=column) from exc
    return Filter(column=column, operator=operator, value=value)


def parse_filters(params: Mapping[str, str], layer: Layer) -> list[Filter]:
    filters = []
    for key, value in params.items():
        if key.lower() in RESERVED_PARAMS:
            continue
        column, operator = key.rsplit("__", 1) if "__" in key else (key, "eq")
        filters.append(_validated_filter(column, operator, value, layer))
    return filters


def _legacy_scalar(value: Any) -> Scalar:
    return value if isinstance(value, (str, bool, int, float)) else str(value)


def compile_filters(filters: list[Filter], layer: Layer) -> tuple[list[sql.Composed], list[Any]]:
    if not filters:
        return [], []
    children: list[Predicate] = []
    for item in filters:
        item = _validated_filter(item.column, item.operator, item.value, layer)
        if item.operator == "in":
            children.append(
                Membership(
                    op="in", field=item.column, value=[_legacy_scalar(v) for v in item.value]
                )
            )
        else:
            children.append(
                Comparison.model_validate(
                    {"op": item.operator, "field": item.column, "value": _legacy_scalar(item.value)}
                )
            )
    fragment, values = compile_predicate(
        Group(op="and", args=children), layer.table_info, legacy_values=True
    )
    return [fragment], values


def compile_query_filters(
    layer: Layer,
    *,
    bbox_srid: int,
    filters: list[Filter] | None = None,
    predicate: Predicate | None = None,
    bbox: BBox | None = None,
) -> tuple[list[sql.Composed], list[Any]]:
    where, params = compile_filters(filters or [], layer)
    if predicate is not None:
        fragment, values = compile_predicate(predicate, layer.table_info)
        where.append(fragment)
        params.extend(values)
    if bbox is not None:
        fragment, values = bbox_predicate(layer.table_info, bbox, bbox_srid)
        where.append(fragment)
        params.extend(values)
    return where, params
