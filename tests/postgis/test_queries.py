"""Queries keep identifiers and values separate, including hostile column names."""

import pytest
from psycopg import sql

from atlas.core.exceptions import InvalidFilter
from atlas.postgis.geometry import to_geojson_feature
from atlas.postgis.introspection import inspect_table
from atlas.postgis.queries import build_feature_query, count_query


@pytest.mark.parametrize("column", ["nombre; DROP TABLE municipios--", "pg_sleep(10)", "geom"])
async def test_bad_columns_rejected(db, column):
    info = await inspect_table(db, "geo", "municipios")
    with pytest.raises(InvalidFilter):
        build_feature_query(info, columns=[column])


@pytest.mark.parametrize("representation", ["geojson", "wkb", "none"])
async def test_representations_and_primary_key(db, representation):
    info = await inspect_table(db, "geo", "escuelas")
    query, params = build_feature_query(
        info, columns=["nombre"], geometry_as=representation, target_srid=4326, limit=1
    )
    row = await db.fetch_one(query, params)
    assert row["id"] == 1 and "nivel" not in row
    if representation == "geojson":
        assert row["geom"]["coordinates"] == pytest.approx([-102.8, 21.3], abs=0.01)
    elif representation == "wkb":
        assert isinstance(row["geom"], bytes)
    else:
        assert "geom" not in row


async def test_parameters_pagination_and_counts(db):
    info = await inspect_table(db, "geo", "municipios")
    predicate = sql.SQL("{} > %s").format(sql.Identifier("poblacion"))
    query, params = build_feature_query(info, where=[predicate], params=[100000], limit=1, offset=1)
    assert params == [100000, 1, 1]
    assert "100000" not in query.as_string()
    assert '"geo"."municipios"' in query.as_string()
    assert len(await db.fetch_all(query, params)) == 1
    query, params = count_query(info, [predicate], [100000])
    assert await db.fetch_one(query, params) == {"count": 3}


def test_feature_conversion_custom_geometry_and_no_id():
    row = {"shape": None, "value": "text"}
    assert to_geojson_feature(row, None, "shape") == {
        "type": "Feature",
        "geometry": None,
        "properties": {"value": "text"},
    }
    assert "shape" in row


async def test_exact_bbox_predicate_excludes_polygon_holes(db):
    from atlas.postgis.queries import intersects_bbox_predicate

    info = await inspect_table(db, "geo", "municipios")
    predicate, values = intersects_bbox_predicate(info, (4, 4, 6, 6), 4326)
    query = sql.SQL(
        "SELECT {} AS hit FROM (SELECT ST_GeomFromText(%s,4326) AS geom) AS data"
    ).format(predicate)
    hole = "POLYGON((0 0,10 0,10 10,0 10,0 0),(3 3,7 3,7 7,3 7,3 3))"
    assert '"geom" && ST_MakeEnvelope' in query.as_string()
    assert await db.fetch_one(query, [*values, hole]) == {"hit": False}


@pytest.mark.parametrize(
    "order_by",
    [
        [("missing", "ASC")],
        [("geom", "ASC")],
        [("id;DROP TABLE x", "ASC")],
        [("id", "DESC;SELECT 1")],
        [("id", "desc")],
        [("id", "ASC"), ("id", "DESC")],
    ],
)
async def test_invalid_order_is_rejected(db, order_by):
    info = await inspect_table(db, "geo", "escuelas")
    with pytest.raises(InvalidFilter):
        build_feature_query(info, order_by=order_by)


async def test_ordered_pages_break_ties_with_primary_key(db):
    info = await inspect_table(db, "geo", "escuelas")
    ids = []
    for offset in range(0, 5, 2):
        query, params = build_feature_query(
            info,
            columns=["nivel"],
            geometry_as="none",
            order_by=[("nivel", "ASC")],
            limit=2,
            offset=offset,
        )
        assert 'ORDER BY "nivel" ASC, "id" ASC LIMIT %s OFFSET %s' in query.as_string()
        ids.extend(row["id"] for row in await db.fetch_all(query, params))
    assert ids == [3, 1, 4, 2, 5]
    assert len(set(ids)) == 5


async def test_explicit_primary_key_direction_and_multiple_criteria(db):
    info = await inspect_table(db, "geo", "escuelas")
    criteria = [("nivel", "ASC"), ("id", "DESC")]
    query, params = build_feature_query(info, geometry_as="none", order_by=criteria)
    assert query.as_string().count('"id" DESC') == 1
    assert '"id" ASC' not in query.as_string()
    assert [row["id"] for row in await db.fetch_all(query, params)] == [3, 4, 1, 5, 2]
    assert criteria == [("nivel", "ASC"), ("id", "DESC")]


async def test_default_order_requires_a_primary_key(db):
    info = await inspect_table(db, "geo", "municipios")
    query, _ = build_feature_query(info)
    assert 'ORDER BY "id" ASC' in query.as_string()
    view = info.model_copy(update={"primary_key": None, "is_view": True})
    query, _ = build_feature_query(view)
    assert "ORDER BY" not in query.as_string()
