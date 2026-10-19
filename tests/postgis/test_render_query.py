"""Rendering queries preserve the spatial index, output CRS and per-layer limits."""

from unittest.mock import AsyncMock

import pytest
from psycopg import sql
from shapely.geometry import Point

from atlas.core.config import AtlasConfig
from atlas.core.exceptions import InvalidFilter
from atlas.postgis.introspection import ColumnInfo, TableInfo, inspect_table
from atlas.postgis.queries import fetch_render_geometries


@pytest.fixture
def info():
    return TableInfo(
        schema_name="geo",
        table_name="escuelas",
        geometry_column="geom",
        geometry_type="Point",
        srid=32613,
        columns=[ColumnInfo(name="id", data_type="int4", nullable=False)],
        primary_key="id",
        is_view=False,
    )


@pytest.mark.parametrize("requested,expected", [(None, 2), (1, 1), (99, 3)])
async def test_bounded_query_and_wkb(info, requested, expected):
    db = AsyncMock()
    db._config = AtlasConfig(database_url="postgresql://test", default_limit=2, max_limit=3)
    db.fetch_all.return_value = [{"geom": Point(1, 2).wkb}, {"geom": None}]
    predicate = sql.SQL("id > %s")
    result = await fetch_render_geometries(
        db,
        info,
        bbox=(-103, 20, -101, 22),
        bbox_srid=4326,
        target_srid=3857,
        where=[predicate],
        params=[10],
        limit=requested,
    )
    query, params = db.fetch_all.call_args.args
    statement = query.as_string()
    assert '"geom" && ST_Transform(ST_MakeEnvelope' in statement
    assert 'ST_AsBinary(ST_Transform("geom", %s))' in statement
    assert "id > %s" in statement and "LIMIT %s" in statement
    assert params == [3857, 10, -103, 20, -101, 22, 4326, 32613, expected, 0]
    assert len(result) == 1 and result[0].equals(Point(1, 2))


async def test_nonpositive_limit_rejected(info):
    db = AsyncMock()
    db._config = AtlasConfig(database_url="postgresql://test")
    with pytest.raises(InvalidFilter):
        await fetch_render_geometries(
            db, info, bbox=(0, 0, 1, 1), bbox_srid=4326, target_srid=3857, limit=0
        )
    db.fetch_all.assert_not_awaited()


async def test_utm_points_reproject_and_filter(db):
    info = await inspect_table(db, "geo", "escuelas")
    result = await fetch_render_geometries(
        db, info, bbox=(-103.1, 20.4, -101.4, 21.6), bbox_srid=4326, target_srid=3857, limit=2
    )
    expected = await db.fetch_one(
        sql.SQL(
            "SELECT ST_X(ST_Transform(geom,3857)) AS x, ST_Y(ST_Transform(geom,3857)) AS y "
            "FROM geo.escuelas WHERE id=1"
        )
    )
    assert len(result) == 2
    assert result[0].x == pytest.approx(expected["x"])
    assert result[0].y == pytest.approx(expected["y"])
    empty = await fetch_render_geometries(
        db, info, bbox=(0, 0, 1, 1), bbox_srid=4326, target_srid=3857
    )
    assert empty == []
