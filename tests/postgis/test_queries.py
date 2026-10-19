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
