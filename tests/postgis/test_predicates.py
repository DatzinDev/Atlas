"""Typed predicates preserve SQL semantics and never interpolate user literals."""

import json
from datetime import date

import pytest
from psycopg import sql

from atlas.core.exceptions import InvalidFilter, UnsupportedCRS
from atlas.core.predicates import Comparison, validate_predicate
from atlas.postgis.introspection import ColumnInfo, TableInfo, inspect_table
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


POLYGON = {
    "type": "Polygon",
    "coordinates": [
        [[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]],
        [[3, 3], [7, 3], [7, 7], [3, 7], [3, 3]],
    ],
}


def spatial(op="within", geometry=POLYGON, srid=4326):
    return predicate({"op": op, "geometry": geometry, "srid": srid})


@pytest.mark.parametrize("op,expected", [("within", [1]), ("intersects", [1, 2, 5])])
@pytest.mark.parametrize("native_srid", [4326, 3857])
async def test_exact_spatial_relation_with_hole(db, op, expected, native_srid):
    metadata = info().model_copy(update={"srid": native_srid, "geometry_column": "shape"})
    geometry = {"type": "MultiPolygon", "coordinates": [POLYGON["coordinates"]]}
    fragment, values = compile_predicate(spatial(op, geometry), metadata)
    query = sql.SQL("""
        SELECT id FROM (
            SELECT id, ST_Transform(ST_SetSRID(ST_MakePoint(x,y),4326), %s) AS shape
            FROM (VALUES (1,1,1),(2,0,1),(3,20,20),(4,4,4),(5,3,4)) p(id,x,y)
        ) candidates WHERE {} ORDER BY id
    """).format(fragment)
    assert [r["id"] for r in await db.fetch_all(query, [native_srid, *values])] == expected
    assert '"shape" && ' in fragment.as_string()
    assert 'ST_Transform("shape"' not in fragment.as_string()
    assert json.dumps(geometry) not in fragment.as_string()


@pytest.mark.parametrize("op", ["within", "intersects"])
async def test_polygon_reprojection_matches_database_reference(db, escuelas_layer, op):
    geojson = json.dumps(
        {
            "type": "Polygon",
            "coordinates": [[[-103, 20.5], [-102, 20.5], [-102, 21.5], [-103, 21.5], [-103, 20.5]]],
        }
    )
    row = await db.fetch_one(
        sql.SQL(
            "SELECT ST_AsGeoJSON("
            "ST_Transform(ST_SetSRID(ST_GeomFromGeoJSON(%s),4326),3857),15,0)::json AS g"
        ).format(),
        [geojson],
    )
    metadata = escuelas_layer.table_info
    fragment, values = compile_predicate(spatial(op, row["g"], 3857), metadata)
    query, params = build_feature_query(
        metadata, where=[fragment], params=values, geometry_as="none"
    )
    actual = [r["id"] for r in await db.fetch_all(query, params)]
    reference = sql.SQL("""
        SELECT id FROM geo.escuelas
        WHERE {}(geom, ST_Transform(ST_SetSRID(ST_GeomFromGeoJSON(%s),4326),32613)) ORDER BY id
    """).format(sql.SQL("ST_Within" if op == "within" else "ST_Intersects"))
    assert actual == [r["id"] for r in await db.fetch_all(reference, [geojson])] == [1, 2, 3, 4]


async def test_spatial_plan_keeps_gist_candidate_filter(db):
    metadata = await inspect_table(db, "geo", "municipios")
    fragment, values = compile_predicate(spatial(), metadata)
    query = sql.SQL("EXPLAIN (FORMAT JSON) SELECT id FROM geo.municipios WHERE {}").format(fragment)
    async with db.connection() as conn:
        await conn.execute("SET LOCAL enable_seqscan = off")
        cursor = await conn.execute(query, values)
        row = await cursor.fetchone()
    plan = row["QUERY PLAN"][0]["Plan"]
    nodes = [plan]
    found = False
    while nodes:
        node = nodes.pop()
        found |= node.get("Index Name") == "municipios_geom_idx" and "&&" in node.get(
            "Index Cond", ""
        )
        nodes.extend(node.get("Plans", []))
    assert found, plan
    with pytest.raises(UnsupportedCRS):
        compile_predicate(spatial(srid=999999), metadata)


async def test_spatial_predicates_compose_with_attributes_and_not(db):
    leaf = {"op": "within", "geometry": POLYGON, "srid": 4326}
    raw = {
        "op": "and",
        "args": [leaf, {"op": "not", "arg": {"op": "eq", "field": "value", "value": "excluded"}}],
    }
    fragment, values = compile_predicate(predicate(raw), info())
    query = sql.SQL("""
        SELECT id FROM (VALUES (1,'keep',ST_SetSRID(ST_MakePoint(1,1),4326)),
                              (2,'excluded',ST_SetSRID(ST_MakePoint(2,2),4326))) p(id,value,geom)
        WHERE {} ORDER BY id
    """).format(fragment)
    assert await db.fetch_all(query, values) == [{"id": 1}]
