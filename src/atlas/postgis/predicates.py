"""Compile validated predicates using catalog identifiers and bound literals."""

from decimal import InvalidOperation
from typing import Any

from psycopg import sql

from atlas.core.exceptions import InvalidFilter
from atlas.core.predicates import Comparison, Group, Membership, Negation, NullCheck, Predicate
from atlas.postgis.introspection import TableInfo
from atlas.postgis.queries import attribute_predicate
from atlas.postgis.values import cast_attribute_value

_TEXT_TYPES = {"text", "varchar", "bpchar", "name"}
_INTEGER_TYPES = {"int2", "int4", "int8"}
_NUMBER_TYPES = {"float4", "float8", "numeric", "decimal"}
_DATE_TYPES = {"date", "timestamp", "timestamptz"}


def _typed_value(value: Any, data_type: str) -> Any:
    valid = (
        (data_type in _TEXT_TYPES | _DATE_TYPES and type(value) is str)
        or (data_type in _INTEGER_TYPES and type(value) is int)
        or (data_type in _NUMBER_TYPES and type(value) in (int, float))
        or (data_type == "bool" and type(value) is bool)
    )
    if not valid:
        raise InvalidFilter("incompatible attribute type")
    try:
        return cast_attribute_value(value, data_type)
    except (ValueError, InvalidOperation, OverflowError) as exc:
        raise InvalidFilter("invalid attribute value") from exc


def compile_predicate(
    predicate: Predicate | None, info: TableInfo
) -> tuple[sql.Composed, list[Any]]:
    """Compile a tree already checked by core.validate_predicate; None means no WHERE."""
    if predicate is None:
        return sql.Composed([]), []
    if isinstance(predicate, Group):
        if predicate.op not in ("and", "or") or not predicate.args:
            raise InvalidFilter("invalid predicate group")
        fragments, values = [], []
        for child in predicate.args:
            if child is None:
                raise InvalidFilter("group child cannot be absent")
            fragment, params = compile_predicate(child, info)
            fragments.append(fragment)
            values.extend(params)
        joiner = sql.SQL(" AND " if predicate.op == "and" else " OR ")
        return sql.SQL("({})").format(joiner.join(fragments)), values
    if isinstance(predicate, Negation):
        if predicate.op != "not" or predicate.arg is None:
            raise InvalidFilter("invalid negation")
        fragment, values = compile_predicate(predicate.arg, info)
        return sql.SQL("(NOT {})").format(fragment), values
    if not isinstance(predicate, (Comparison, Membership, NullCheck)):
        raise InvalidFilter("invalid attribute predicate")
    columns = {c.name: c for c in info.columns if c.name != info.geometry_column}
    if predicate.field not in columns:
        raise InvalidFilter("unknown attribute", column=predicate.field)
    column = sql.Identifier(predicate.field)
    if isinstance(predicate, NullCheck):
        operators = {"is_null": sql.SQL("IS NULL"), "is_not_null": sql.SQL("IS NOT NULL")}
        if predicate.op not in operators:
            raise InvalidFilter("unknown null operator")
        return sql.SQL("({} {})").format(column, operators[predicate.op]), []
    kind = columns[predicate.field].data_type
    if predicate.op in ("like", "ilike") and kind not in _TEXT_TYPES:
        raise InvalidFilter("LIKE/ILIKE requires a text attribute")
    if isinstance(predicate, Membership):
        if predicate.op != "in" or not isinstance(predicate.value, list) or not predicate.value:
            raise InvalidFilter("IN requires a non-empty list")
        value = [_typed_value(v, kind) for v in predicate.value]
    else:
        value = _typed_value(predicate.value, kind)
    if predicate.op == "ilike":
        return sql.SQL("({} ILIKE %s)").format(column), [value]
    fragment, values = attribute_predicate(info, predicate.field, predicate.op, value)
    return sql.SQL("({})").format(fragment), values
