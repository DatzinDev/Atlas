"""Typed predicates preserve SQL semantics and never interpolate user literals."""

from datetime import date

import pytest
from psycopg import sql

from atlas.core.exceptions import InvalidFilter
from atlas.core.predicates import Comparison, validate_predicate
from atlas.postgis.introspection import ColumnInfo, TableInfo
from atlas.postgis.predicates import compile_predicate
from atlas.postgis.queries import build_feature_query
from atlas.wfs.filters import compile_filters, parse_filters


def predicate(raw):
    return validate_predicate(raw, max_depth=8, max_nodes=100)


def info(kind="text", field="value"):
    return TableInfo(
        schema_name="geo",
        table_name="test",
        geometry_column="geom",
        geometry_type="Point",
        srid=4326,
        primary_key=None,
        is_view=False,
        columns=[ColumnInfo(name=field, data_type=kind, nullable=True)],
    )


@pytest.mark.parametrize(
    "op,value",
    [
        ("eq", 100000),
        ("ne", 100000),
        ("gt", 100000),
        ("gte", 100000),
        ("lt", 100000),
        ("lte", 100000),
        ("in", [100000, 250000]),
        ("like", "%Villa%"),
    ],
)
async def test_legacy_equivalence(db, municipios_layer, op, value):
    field = "nombre" if op == "like" else "poblacion"
    new, params = compile_predicate(
        predicate({"op": op, "field": field, "value": value}), municipios_layer.table_info
    )
    legacy, legacy_params = compile_filters(
        parse_filters(
            {
                f"{field}__{op}": ",".join(map(str, value))
                if isinstance(value, list)
                else str(value)
            },
            municipios_layer,
        ),
        municipios_layer,
    )

    async def rows(where, values):
        q, p = build_feature_query(
            municipios_layer.table_info, where=where, params=values, geometry_as="none"
        )
        return await db.fetch_all(q, p)

    assert await rows([new], params) == await rows(legacy, legacy_params)


@pytest.mark.parametrize(
    "op,expected",
    [
        ("is_null", [3]),
        ("is_not_null", [1, 2]),
        ("not", [2]),
        ("and", [1]),
        ("or", [1, 3]),
        ("ilike", [1, 2]),
    ],
)
async def test_boolean_null_and_pattern_semantics(db, op, expected):
    eq = {"op": "eq", "field": "value", "value": "Alpha"}
    null = {"op": "is_null", "field": "value"}
    raw = {"op": op, "field": "value"}
    if op == "not":
        raw = {"op": op, "arg": eq}
    if op == "and":
        raw = {"op": op, "args": [eq, {"op": "not", "arg": null}]}
    if op == "or":
        raw = {"op": op, "args": [eq, null]}
    if op == "ilike":
        raw["value"] = "%alpha%"
    fragment, params = compile_predicate(predicate(raw), info())
    query = sql.SQL(
        "SELECT id FROM (VALUES (1,'Alpha'::text),(2,'ALPHA'),(3,NULL)) t(id,value) "
        "WHERE {} ORDER BY id"
    ).format(fragment)
    assert [r["id"] for r in await db.fetch_all(query, params)] == expected


@pytest.mark.parametrize(
    "kind,value",
    [
        ("int2", 32768),
        ("int4", 2147483648),
        ("int8", 9223372036854775808),
        ("int4", True),
        ("int4", "1"),
        ("bool", 1),
        ("bool", "true"),
        ("text", 1),
        ("float4", 3.5e38),
        ("date", "2026-02-30"),
        ("uuid", "x"),
    ],
)
def test_rejects_incompatible_or_out_of_range_types(kind, value):
    with pytest.raises(InvalidFilter):
        compile_predicate(predicate({"op": "eq", "field": "value", "value": value}), info(kind))


def test_dates_and_hostile_names_operators_values():
    fragment, values = compile_predicate(
        predicate({"op": "eq", "field": "value", "value": "2026-01-02"}), info("date")
    )
    assert values == [date(2026, 1, 2)]
    payload = "x' OR 1=1;--"
    fragment, values = compile_predicate(
        predicate({"op": "eq", "field": "value", "value": payload}), info()
    )
    assert payload not in fragment.as_string() and values == [payload]
    for field in ("geom", "missing", "value;DROP TABLE x"):
        with pytest.raises(InvalidFilter):
            compile_predicate(Comparison(op="eq", field=field, value="x"), info())
    with pytest.raises(InvalidFilter):
        compile_predicate(
            Comparison.model_construct(op="=;SELECT 1", field="value", value="x"), info()
        )
    with pytest.raises(InvalidFilter):
        compile_predicate(Comparison(op="ilike", field="value", value="%"), info("int4"))
    assert compile_predicate(None, info())[0].as_string() == ""
