"""Bounding boxes transform the envelope, keeping spatial indexes usable."""

import pytest
from psycopg import sql

from atlas.core.exceptions import InvalidGeometry
from atlas.postgis.introspection import inspect_table
from atlas.postgis.queries import bbox_predicate, build_feature_query, parse_bbox


@pytest.mark.parametrize(
    "value", ["1,2,3", "a,2,3,4", "3,2,1,4", "1,4,3,2", "nan,2,3,4", "1,2,inf,4", "1,2,1,4"]
)
def test_invalid_bbox(value):
    with pytest.raises(InvalidGeometry):
        parse_bbox(value)


def test_parse_optional_crs():
    assert parse_bbox("-103,20,-102,21,EPSG:4326") == ((-103, 20, -102, 21), 4326)
    assert parse_bbox("-103,20,-102,21")[1] is None


async def test_bbox_on_utm_table(db):
    info = await inspect_table(db, "geo", "escuelas")
    predicate, params = bbox_predicate(info, (-102.9, 21.1, -102.5, 21.4), 4326)
    assert predicate.as_string().startswith('"geom" && ST_Transform(ST_MakeEnvelope(')
    assert params == [-102.9, 21.1, -102.5, 21.4, 4326, 32613]
    query, values = build_feature_query(info, where=[predicate], params=params, limit=5)
    assert [row["id"] for row in await db.fetch_all(query, values)] == [1]


async def test_spatial_index_is_eligible(db):
    info = await inspect_table(db, "geo", "municipios")
    predicate, params = bbox_predicate(info, (-102.9, 21.1, -102.8, 21.2), 4326)
    query, values = build_feature_query(info, where=[predicate], params=params, limit=5)
    async with db.connection() as conn:
        # The tiny seed favors a sequential scan; force the eligible spatial access path.
        await conn.execute("SET LOCAL enable_seqscan = off")
        cursor = await conn.execute(sql.SQL("EXPLAIN (FORMAT JSON) {}").format(query), values)
        plan = await cursor.fetchone()
    assert "municipios_geom_idx" in str(plan)
