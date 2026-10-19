"""Filter casting and hostile input must not change SQL structure."""

from datetime import date

import pytest

from atlas.core.exceptions import InvalidFilter
from atlas.postgis.introspection import ColumnInfo
from atlas.postgis.queries import build_feature_query
from atlas.wfs.filters import Filter, compile_filters, parse_filters


@pytest.mark.parametrize(
    "key,value",
    [
        ("poblacion__union", "1"),
        ('"geom"__eq', "x"),
        ("pg_sleep(10)", "x"),
        ("poblacion__gt", "abc"),
        ("poblacion__in", "1,,2"),
        ("poblacion__eq", "99999999999999999999"),
        ("poblacion__like", "%"),
    ],
)
async def test_invalid_filters(municipios_layer, key, value):
    with pytest.raises(InvalidFilter):
        parse_filters({key: value}, municipios_layer)


@pytest.mark.parametrize("operator", ["eq", "ne", "gt", "gte", "lt", "lte", "in", "like"])
async def test_all_operators(db, municipios_layer, operator):
    key = "nombre" if operator == "like" else "poblacion"
    value = "%" if operator == "like" else "100000,200000" if operator == "in" else "100000"
    filters = parse_filters({f"{key}__{operator}": value}, municipios_layer)
    where, params = compile_filters(filters, municipios_layer)
    query, values = build_feature_query(
        municipios_layer.table_info, where=where, params=params, limit=5
    )
    await db.fetch_all(query, values)
    assert params == ["%"] if operator == "like" else value not in query.as_string()


async def test_hostile_values_are_parameters(db, municipios_layer):
    payload = "x' OR '1'='1"
    where, params = compile_filters(
        parse_filters({"nombre__eq": payload}, municipios_layer), municipios_layer
    )
    query, values = build_feature_query(
        municipios_layer.table_info, where=where, params=params, limit=5
    )
    assert payload not in query.as_string() and params == [payload]
    assert await db.fetch_all(query, values) == []
    assert parse_filters({"SERVICE": "WFS", "count": "1"}, municipios_layer) == []
    with pytest.raises(InvalidFilter):
        compile_filters([Filter(column="geom", operator="eq", value="bad")], municipios_layer)


@pytest.mark.parametrize(
    "kind,value,expected",
    [
        ("bool", "false", False),
        ("date", "2026-01-02", date(2026, 1, 2)),
        ("float8", "1.25", 1.25),
        ("text", "hello", "hello"),
    ],
)
async def test_scalar_types(municipios_layer, kind, value, expected):
    municipios_layer.attributes = [ColumnInfo(name="value", data_type=kind, nullable=True)]
    assert parse_filters({"value": value}, municipios_layer)[0].value == expected
