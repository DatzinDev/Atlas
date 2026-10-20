"""Predicate grammar, literal safety and budgets before recursive validation."""

import pytest
from pydantic import ValidationError

from atlas.core.config import AtlasConfig
from atlas.core.exceptions import InvalidFilter
from atlas.core.predicates import Comparison, validate_predicate


def validate(value, **limits):
    return validate_predicate(
        value, max_depth=limits.get("depth", 8), max_nodes=limits.get("nodes", 100)
    )


def test_nested_boolean_and_literals():
    literal = "'; DROP TABLE users;--"
    raw = {
        "op": "and",
        "args": [
            {"op": "eq", "field": "nombre", "value": literal},
            {
                "op": "or",
                "args": [
                    {"op": "is_null", "field": "baja"},
                    {"op": "not", "arg": {"op": "ilike", "field": "nombre", "value": "%x_"}},
                ],
            },
        ],
    }
    predicate = validate(raw)
    assert predicate.model_dump() == raw
    assert validate(predicate) == predicate
    assert validate(None) is None


@pytest.mark.parametrize("op", ["eq", "ne", "gt", "gte", "lt", "lte", "like", "ilike"])
@pytest.mark.parametrize("value", ["text", True, 123, 1.5])
def test_scalar_types_are_preserved(op, value):
    result = validate({"op": op, "field": "value", "value": value})
    assert type(result.value) is type(value)


@pytest.mark.parametrize(
    "raw",
    [
        {"op": "or", "args": []},
        {"op": "and", "args": []},
        {"op": "not"},
        {"op": "unknown", "field": "x", "value": 1},
        {"op": "eq", "field": "x", "value": None},
        {"op": "eq", "field": "x", "value": float("nan")},
        {"op": "in", "field": "x", "value": [float("inf")]},
        {"op": "in", "field": "x", "value": []},
        {"op": "in", "field": "x", "value": [None]},
        {"op": "eq", "field": "x", "value": {"sql": "true"}},
        {"op": "is_null", "field": "x", "value": 1},
        {"op": "and", "args": [{"op": "is_null", "field": "x"}], "field": "x"},
        {"op": "eq", "field": "", "value": 1},
        [],
    ],
)
def test_rejects_ambiguous_or_invalid_payloads(raw):
    with pytest.raises(InvalidFilter):
        validate(raw)


def test_exact_limits_and_deep_or_cyclic_input():
    leaf = {"op": "is_not_null", "field": "x"}
    assert validate({"op": "and", "args": [leaf, leaf]}, depth=2, nodes=3)
    with pytest.raises(InvalidFilter):
        validate({"op": "and", "args": [leaf, leaf]}, nodes=2)
    deep = leaf
    for _ in range(2000):
        deep = {"op": "not", "arg": deep}
    with pytest.raises(InvalidFilter):
        validate(deep)
    cycle = {"op": "not"}
    cycle["arg"] = cycle
    with pytest.raises(InvalidFilter):
        validate(cycle)
    assert validate({"op": "not", "arg": leaf}, depth=2)
    with pytest.raises(InvalidFilter):
        validate({"op": "not", "arg": leaf}, depth=1)


def test_config_and_python_models_are_validated():
    config = AtlasConfig(database_url="postgresql://test")
    assert (config.filter_max_depth, config.filter_max_nodes) == (8, 100)
    assert validate({"op": "in", "field": "x", "value": [1, "2", False]})
    forged = Comparison.model_construct(op="eq", field="x", value=float("inf"))
    with pytest.raises(InvalidFilter):
        validate(forged)
    for name in ("filter_max_depth", "filter_max_nodes"):
        for value in (0, -1):
            with pytest.raises(ValidationError):
                AtlasConfig(database_url="test", **{name: value})
    with pytest.raises(InvalidFilter):
        validate_predicate(None, max_depth=0, max_nodes=1)
