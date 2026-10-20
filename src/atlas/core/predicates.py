"""Bounded attribute predicates, independent of transport and database types."""

from __future__ import annotations

from typing import Annotated, Any, Literal

from pydantic import BaseModel, ConfigDict, Field, TypeAdapter, ValidationError

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


Predicate = Annotated[
    Comparison | Membership | NullCheck | Group | Negation, Field(discriminator="op")
]
_ADAPTER: TypeAdapter[Predicate] = TypeAdapter(Predicate)


def validate_predicate(value: Any, *, max_depth: int, max_nodes: int) -> Predicate | None:
    """Check tree budgets iteratively before Pydantic's recursive validation."""
    if type(max_depth) is not int or type(max_nodes) is not int or min(max_depth, max_nodes) < 1:
        raise InvalidFilter("predicate limits must be positive integers")
    if value is None:
        return None
    stack = [(value, 1)]
    nodes = 0
    while stack:
        node, depth = stack.pop()
        nodes += 1
        if depth > max_depth or nodes > max_nodes:
            raise InvalidFilter("predicate exceeds depth or node limit")
        if isinstance(node, PredicateModel):
            node = {key: getattr(node, key) for key in type(node).model_fields}
        if not isinstance(node, dict):
            raise InvalidFilter("predicate must be an object")
        if node.get("op") in ("and", "or"):
            children = node.get("args")
            if not isinstance(children, list) or not children:
                raise InvalidFilter("predicate group requires non-empty args")
            if nodes + len(stack) + len(children) > max_nodes:
                raise InvalidFilter("predicate exceeds node limit")
            stack.extend((child, depth + 1) for child in children)
        elif node.get("op") == "not":
            stack.append((node.get("arg"), depth + 1))
    try:
        return _ADAPTER.validate_python(value)
    except (ValidationError, RecursionError) as exc:
        raise InvalidFilter("invalid predicate") from exc
