"""Parse whitelisted attribute filters before their values reach PostgreSQL."""

import math
from collections.abc import Mapping
from datetime import date, datetime
from decimal import Decimal, InvalidOperation
from typing import Any

from psycopg import sql
from pydantic import BaseModel

from atlas.core.exceptions import InvalidFilter
from atlas.layers.models import Layer
from atlas.postgis.queries import FILTER_OPERATORS, attribute_predicate

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


def _cast(value: Any, data_type: str) -> Any:
    text = str(value)
    if data_type in ("int2", "int4", "int8"):
        bits = {"int2": 16, "int4": 32, "int8": 64}[data_type]
        number = int(text)
        if not -(2 ** (bits - 1)) <= number < 2 ** (bits - 1):
            raise ValueError("integer out of range")
        return number
    if data_type in ("float4", "float8"):
        number_float = float(text)
        if not math.isfinite(number_float) or (
            data_type == "float4" and abs(number_float) > 3.4028235e38
        ):
            raise ValueError("non-finite number")
        return number_float
    if data_type in ("numeric", "decimal"):
        number_decimal = Decimal(text)
        if not number_decimal.is_finite():
            raise ValueError("non-finite number")
        return number_decimal
    if data_type == "bool":
        if text.lower() not in ("true", "false", "1", "0"):
            raise ValueError("invalid boolean")
        return text.lower() in ("true", "1")
    if data_type == "date":
        return date.fromisoformat(text)
    if data_type in ("timestamp", "timestamptz"):
        return datetime.fromisoformat(text)
    return text


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
                _cast(v.strip() if isinstance(v, str) else v, attributes[column].data_type)
                for v in values
            ]
        else:
            value = _cast(value, attributes[column].data_type)
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


def compile_filters(filters: list[Filter], layer: Layer) -> tuple[list[sql.Composed], list[Any]]:
    predicates, values = [], []
    for item in filters:
        item = _validated_filter(item.column, item.operator, item.value, layer)
        predicate, params = attribute_predicate(
            layer.table_info, item.column, item.operator, item.value
        )
        predicates.append(predicate)
        values.extend(params)
    return predicates, values
