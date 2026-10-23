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


@pytest.mark.parametrize("requested,expected", [(None, 3), (1, 1), (99, 3)])
async def test_bounded_query_and_wkb(info, requested, expected):
    from unittest.mock import Mock

    db = Mock()
    db._config = AtlasConfig(
        database_url="postgresql://test", default_limit=1, max_limit=1, max_render_features=3
    )

    async def rows(*args, **kwargs):
        yield {"size": 21, "data": Point(1, 2).wkb}
        yield {"size": None, "data": None}

    db.stream.side_effect = rows
    result = await fetch_render_geometries(
        db,
        info,
        bbox=(-103, 20, -101, 22),
        bbox_srid=4326,
        target_srid=3857,
        where=[sql.SQL("id > %s")],
        params=[10],
        limit=requested,
    )
    query, values = db.stream.call_args.args
    assert '"geom" && ST_Transform(ST_MakeEnvelope' in query.as_string()
    assert 'ST_AsBinary(ST_Transform("geom", %s))' in query.as_string()
    assert values == [67108864, 3857, 10, -103, 20, -101, 22, 4326, 32613, expected + 1, 0]
    assert db.stream.call_args.kwargs == {}
    assert "ROWS UNBOUNDED PRECEDING" in query.as_string()
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
        db, info, bbox=(-103.1, 20.4, -101.4, 21.6), bbox_srid=4326, target_srid=3857, limit=100
    )
    expected = await db.fetch_one(
        sql.SQL(
            "SELECT ST_X(ST_Transform(geom,3857)) AS x, ST_Y(ST_Transform(geom,3857)) AS y "
            "FROM geo.escuelas WHERE id=1"
        )
    )
    assert len(result) == 5
    assert result[0].x == pytest.approx(expected["x"])
    assert result[0].y == pytest.approx(expected["y"])
    empty = await fetch_render_geometries(
        db, info, bbox=(0, 0, 1, 1), bbox_srid=4326, target_srid=3857
    )
    assert empty == []


@pytest.mark.parametrize(
    "count,cap,byte_cap,success",
    [
        (1000, 1001, 21021, True),
        (1001, 1001, 21021, True),
        (1002, 1001, 999999, False),
        (2, 10, 42, True),
        (2, 10, 41, False),
        (1, 10, 20, False),
    ],
)
async def test_budget_boundaries_close_stream(info, count, cap, byte_cap, success):
    from unittest.mock import Mock

    from atlas.core.exceptions import QueryTooLarge
    from atlas.postgis.queries import RenderBudget

    closed = []

    async def rows(*args, **kwargs):
        try:
            for _ in range(count):
                yield {"size": 21, "data": Point(1, 2).wkb}
        finally:
            closed.append(True)

    db = Mock(_config=AtlasConfig(database_url="unused", max_render_features=cap))
    db.stream.side_effect = rows
    budget = RenderBudget(cap, byte_cap)
    call = fetch_render_geometries(
        db, info, bbox=(0, 0, 1, 1), bbox_srid=3857, target_srid=3857, budget=budget
    )
    if success:
        assert len(await call) == count
        assert budget.features == cap - count and budget.bytes == byte_cap - 21 * count
    else:
        with pytest.raises(QueryTooLarge):
            await call
    assert closed == [True]


async def test_real_byte_overflow_recovers_pool(db):
    from atlas.core.exceptions import QueryTooLarge
    from atlas.postgis.queries import RenderBudget

    info = await inspect_table(db, "geo", "escuelas")
    with pytest.raises(QueryTooLarge):
        await fetch_render_geometries(
            db,
            info,
            bbox=(-104, 19, -100, 23),
            bbox_srid=4326,
            target_srid=3857,
            budget=RenderBudget(100, 1),
        )
    assert (await db.fetch_one(sql.SQL("SELECT 1 AS ok")))["ok"] == 1


async def test_sql_bounds_entire_batch_payload(db):
    from unittest.mock import patch

    from atlas.core.exceptions import QueryTooLarge
    from atlas.postgis.queries import RenderBudget

    info = await inspect_table(db, "geo", "escuelas")
    with patch.object(db, "stream", wraps=db.stream) as stream, pytest.raises(QueryTooLarge):
        await fetch_render_geometries(
            db,
            info,
            bbox=(-104, 19, -100, 23),
            bbox_srid=4326,
            target_srid=3857,
            budget=RenderBudget(100, 42),
        )
    query, values = stream.call_args.args
    rows = await db.fetch_all(query, values)
    assert sum(len(row["data"]) for row in rows if row["data"] is not None) == 42
    assert len(rows) > 2 and all(row["data"] is None for row in rows[2:])
