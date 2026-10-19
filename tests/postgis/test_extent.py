"""Bounding boxes, reprojection, estimated statistics and exact fallback in real PostGIS."""

from collections.abc import AsyncIterator
from unittest.mock import AsyncMock

import pytest
from psycopg import sql

from atlas.core.exceptions import InvalidParameter
from atlas.postgis.connection import Database
from atlas.postgis.introspection import TableInfo, inspect_table, table_extent


@pytest.fixture
async def extent_table(db: Database) -> AsyncIterator[TableInfo]:
    async with db.connection() as conn:
        await conn.execute("""
            CREATE TABLE geo.extent_cases (geom geometry(Point, 4326))
            WITH (autovacuum_enabled = false);
            INSERT INTO geo.extent_cases VALUES
                (ST_SetSRID(ST_MakePoint(-103, 20), 4326)),
                (ST_SetSRID(ST_MakePoint(-101, 22), 4326));
        """)
    try:
        yield await inspect_table(db, "geo", "extent_cases")
    finally:
        async with db.connection() as conn:
            await conn.execute("DROP TABLE geo.extent_cases")


async def test_missing_statistics_use_exact_extent(
    db: Database, extent_table: TableInfo, monkeypatch: pytest.MonkeyPatch
) -> None:
    fetch = AsyncMock(wraps=db.fetch_one)
    monkeypatch.setattr(db, "fetch_one", fetch)
    assert await table_extent(db, extent_table) == (-103, 20, -101, 22)
    assert fetch.await_count == 2


async def test_statistics_avoid_exact_scan(
    db: Database, extent_table: TableInfo, monkeypatch: pytest.MonkeyPatch
) -> None:
    async with db.connection() as conn:
        await conn.execute("ANALYZE geo.extent_cases")
    fetch = AsyncMock(wraps=db.fetch_one)
    monkeypatch.setattr(db, "fetch_one", fetch)
    extent = await table_extent(db, extent_table)
    assert extent == pytest.approx((-103, 20, -101, 22), abs=0.1)
    assert fetch.await_count == 1


@pytest.mark.parametrize("contents", ["", "NULL", "ST_GeomFromText('POINT EMPTY', 4326)"])
async def test_no_geometries_return_none_even_with_stale_statistics(
    db: Database, extent_table: TableInfo, contents: str
) -> None:
    async with db.connection() as conn:
        await conn.execute("ANALYZE geo.extent_cases; DELETE FROM geo.extent_cases")
        if contents:
            # Fixed test expressions only; no user input enters this SQL.
            await conn.execute(
                sql.SQL("INSERT INTO geo.extent_cases VALUES ({})").format(sql.SQL(contents))
            )
    assert await table_extent(db, extent_table) is None


async def test_view_extent(db: Database) -> None:
    info = await inspect_table(db, "geo", "v_municipios_grandes")
    assert await table_extent(db, info) == (-103, 20.5, -101.5, 21.5)


async def test_utm_table_in_default_srid(db: Database) -> None:
    info = await inspect_table(db, "geo", "escuelas")
    assert await table_extent(db, info) == pytest.approx((-102.8, 20.7, -101.75, 21.3), abs=0.03)


async def test_utm_points_are_reprojected_before_exact_aggregation(db: Database) -> None:
    async with db.connection() as conn:
        await conn.execute("CREATE VIEW geo.extent_utm AS SELECT geom FROM geo.escuelas")
    try:
        info = await inspect_table(db, "geo", "extent_utm")
        extent = await table_extent(db, info)
        assert extent == pytest.approx((-102.8, 20.7, -101.75, 21.3), abs=0.01)
    finally:
        async with db.connection() as conn:
            await conn.execute("DROP VIEW geo.extent_utm")


async def test_requested_srid(db: Database, extent_table: TableInfo) -> None:
    extent = await table_extent(db, extent_table, 3857)
    assert extent == pytest.approx((-11465907.55, 2273030.93, -11243268.57, 2511525.23), abs=0.01)


async def test_invalid_srid_is_rejected(db: Database, extent_table: TableInfo) -> None:
    with pytest.raises(InvalidParameter):
        await table_extent(db, extent_table, 0)
