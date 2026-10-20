"""Predicate grammar, literal safety and budgets before recursive validation."""

import pytest
from pydantic import ValidationError

from atlas.core.config import AtlasConfig
from atlas.core.exceptions import InvalidFilter
from atlas.core.predicates import Comparison, parse_predicate, validate_predicate


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


SQUARE = {"type": "Polygon", "coordinates": [[[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]]]}


def spatial(geometry=SQUARE, **kwargs):
    return {"op": "within", "geometry": geometry, "srid": 4326, **kwargs}


def test_valid_polygons_multipolygons_and_holes():
    hole = [[2, 2], [2, 4], [4, 4], [4, 2], [2, 2]]
    polygon = {"type": "Polygon", "coordinates": [*SQUARE["coordinates"], hole]}
    multi = {"type": "MultiPolygon", "coordinates": [polygon["coordinates"]]}
    for geometry in (SQUARE, polygon, multi):
        raw = spatial(geometry)
        assert validate(raw).model_dump() == raw
        assert validate(validate(raw)).model_dump() == raw


@pytest.mark.parametrize(
    "geometry",
    [
        {"type": "GeometryCollection", "geometries": []},
        {"type": "Polygon", "coordinates": []},
        {"type": "MultiPolygon", "coordinates": [[]]},
        {"type": "Polygon", "coordinates": [[[0, 0], [1, 1], [1, 0], [0, 1], [0, 0]]]},
        {"type": "Polygon", "coordinates": [[[0, 0], [1, 0], [1, 1], [0, 1]]]},
        {"type": "Polygon", "coordinates": [[[0, 0, 1]] * 4]},
        {"type": "Polygon", "coordinates": [[[0, 0], [True, 0], [1, 1], [0, 0]]]},
        {"type": "Polygon", "coordinates": [[[0, 0], [float("inf"), 0], [1, 1], [0, 0]]]},
        {**SQUARE, "crs": "EPSG:4326"},
    ],
)
def test_invalid_spatial_geometry(geometry):
    with pytest.raises(InvalidFilter):
        validate(spatial(geometry))


@pytest.mark.parametrize(
    "extra", [{"srid": 0}, {"srid": -1}, {"srid": True}, {"srid": "4326"}, {"column": "geom"}]
)
def test_spatial_srid_and_extra_keys(extra):
    with pytest.raises(InvalidFilter):
        validate(spatial(**extra))
    with pytest.raises(InvalidFilter):
        validate({"op": "within", "geometry": SQUARE})


def test_spatial_budgets_before_native_validation(monkeypatch):
    import json

    import atlas.core.predicates as module

    raw = spatial()
    size = len(json.dumps(raw, separators=(",", ":")).encode())
    assert validate_predicate(raw, max_depth=1, max_nodes=1, max_bytes=size, max_coordinates=5)

    def unexpected(*args):
        raise AssertionError("GEOS must not run for over-budget input")

    monkeypatch.setattr(module, "shape", unexpected)
    for limits in ({"max_coordinates": 4}, {"max_bytes": size - 1}):
        with pytest.raises(InvalidFilter):
            validate_predicate(raw, max_depth=1, max_nodes=1, **limits)
    with pytest.raises(InvalidFilter):
        validate_predicate(
            {"op": "and", "args": [raw, raw]}, max_depth=2, max_nodes=3, max_coordinates=9
        )
    config = AtlasConfig(database_url="test")
    assert (config.filter_max_bytes, config.filter_max_coordinates) == (65536, 10000)
    for name in ("filter_max_bytes", "filter_max_coordinates"):
        with pytest.raises(ValidationError):
            AtlasConfig(database_url="test", **{name: 0})


@pytest.mark.parametrize(
    "raw",
    [
        "",
        "{",
        "[]",
        '"sql"',
        '{"op":"eq","op":"ne","field":"x","value":1}',
        '{"op":"and","args":[{"op":"eq","field":"x","value":1,"value":2}]}',
        '{"op":"eq","field":"x","value":NaN}',
        '{"op":"eq","field":"x","value":Infinity}',
        '{"op":"eq","field":"x","value":1e999}',
        "[" * 2000 + "]" * 2000,
    ],
)
def test_json_parser_rejects_invalid_and_ambiguous_json(raw):
    with pytest.raises(InvalidFilter):
        parse_predicate(raw, AtlasConfig(database_url="test"))


def test_parser_checks_utf8_bytes_before_json(monkeypatch):
    import json

    config = AtlasConfig(database_url="test", filter_max_bytes=4)

    def unexpected(*args, **kwargs):
        raise AssertionError("JSON decoder must not run for oversized input")

    monkeypatch.setattr(json, "loads", unexpected)
    for raw in (" " * 5, "á" * 3):
        with pytest.raises(InvalidFilter):
            parse_predicate(raw, config)


def test_parser_uses_config_for_every_budget():
    import json

    config = AtlasConfig(database_url="test")
    raw = {"op": "eq", "field": "nombre", "value": "Jaliscó"}
    assert parse_predicate(json.dumps(raw, ensure_ascii=False), config).model_dump() == raw
    assert parse_predicate(None, config) is None and parse_predicate("null", config) is None
    group = json.dumps({"op": "not", "arg": raw})
    for limits in ({"filter_max_depth": 1}, {"filter_max_nodes": 1}):
        with pytest.raises(InvalidFilter):
            parse_predicate(group, config.model_copy(update=limits))
    with pytest.raises(InvalidFilter):
        parse_predicate(
            json.dumps(spatial()), config.model_copy(update={"filter_max_coordinates": 4})
        )
    assert parse_predicate('{"op":"in","field":"x","value":[1,2]}', config)
    with pytest.raises(InvalidFilter):
        parse_predicate(
            '{"op":"in","field":"x","value":[1,2]}',
            config.model_copy(update={"filter_max_in_values": 1}),
        )
    assert config.filter_max_in_values == 1000
    with pytest.raises(ValidationError):
        AtlasConfig(database_url="test", filter_max_in_values=0)
    assert validate_predicate(
        {"op": "in", "field": "x", "value": [0] * 1000}, max_depth=1, max_nodes=1
    )
    with pytest.raises(InvalidFilter):
        validate_predicate(
            {"op": "in", "field": "x", "value": [0] * 1001}, max_depth=1, max_nodes=1
        )
