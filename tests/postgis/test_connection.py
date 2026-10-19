"""Pool lifecycle, row shape, statement timeout, server-side streaming and error translation."""

from __future__ import annotations

from collections.abc import AsyncIterator

import psycopg
import pytest
from psycopg import sql

from atlas.core.config import AtlasConfig
from atlas.core.exceptions import DatabaseError, QueryTimeout
from atlas.postgis.connection import Database


@pytest.fixture
async def impatient_db(database_url: str) -> AsyncIterator[Database]:
    """A database whose statement timeout is short enough to trip on purpose."""
    database = Database(AtlasConfig(database_url=database_url, query_timeout=0.1))
    await database.open()
    try:
        yield database
    finally:
        await database.close()


def query(text: str) -> sql.Composed:
    return sql.SQL(text).format()


class TestLifecycle:
    async def test_open_twice_keeps_the_same_pool(self, db: Database) -> None:
        pool = db.pool

        await db.open()

        assert db.pool is pool

    async def test_close_is_idempotent(self, database_url: str) -> None:
        database = Database(AtlasConfig(database_url=database_url))
        await database.open()

        await database.close()
        await database.close()

        assert database.pool is None

    async def test_close_without_open_is_a_no_op(self, database_url: str) -> None:
        await Database(AtlasConfig(database_url=database_url)).close()

    async def test_querying_a_closed_database_is_reported_as_a_database_error(
        self, database_url: str
    ) -> None:
        database = Database(AtlasConfig(database_url=database_url))

        with pytest.raises(DatabaseError):
            await database.fetch_all(query("SELECT 1"))

    async def test_reopening_after_close_works(self, database_url: str) -> None:
        database = Database(AtlasConfig(database_url=database_url))
        await database.open()
        await database.close()
        await database.open()
        try:
            assert await database.fetch_one(query("SELECT 1 AS n")) == {"n": 1}
        finally:
            await database.close()

    async def test_the_pool_is_sized_from_the_config(self, database_url: str) -> None:
        database = Database(
            AtlasConfig(database_url=database_url, pool_min_size=2, pool_max_size=4)
        )
        await database.open()
        try:
            assert (database.pool.min_size, database.pool.max_size) == (2, 4)
        finally:
            await database.close()


class TestRows:
    async def test_fetch_all_returns_dicts(self, db: Database) -> None:
        rows = await db.fetch_all(query("SELECT 1 AS n, 'a' AS letter"))

        assert rows == [{"n": 1, "letter": "a"}]

    async def test_fetch_all_returns_an_empty_list_when_nothing_matches(self, db: Database) -> None:
        assert await db.fetch_all(query("SELECT 1 WHERE false")) == []

    async def test_fetch_one_returns_none_when_nothing_matches(self, db: Database) -> None:
        assert await db.fetch_one(query("SELECT 1 AS n WHERE false")) is None

    async def test_values_travel_as_parameters(self, db: Database) -> None:
        row = await db.fetch_one(query("SELECT %s::int AS n"), [42])

        assert row == {"n": 42}


class TestStatementTimeout:
    async def test_the_timeout_is_applied_to_every_pooled_connection(self, db: Database) -> None:
        row = await db.fetch_one(query("SHOW statement_timeout"))

        assert row == {"statement_timeout": "30s"}

    async def test_a_slow_query_raises_query_timeout(self, impatient_db: Database) -> None:
        with pytest.raises(QueryTimeout):
            await impatient_db.fetch_all(query("SELECT pg_sleep(3)"))

    async def test_the_connection_is_usable_again_after_a_timeout(
        self, impatient_db: Database
    ) -> None:
        with pytest.raises(QueryTimeout):
            await impatient_db.fetch_all(query("SELECT pg_sleep(3)"))

        assert await impatient_db.fetch_one(query("SELECT 1 AS n")) == {"n": 1}


class TestErrorTranslation:
    async def test_a_driver_error_becomes_a_database_error(self, db: Database) -> None:
        with pytest.raises(DatabaseError) as raised:
            await db.fetch_all(query("SELECT * FROM table_that_does_not_exist"))

        assert not isinstance(raised.value, psycopg.Error)

    async def test_the_driver_message_stays_out_of_the_public_message(self, db: Database) -> None:
        with pytest.raises(DatabaseError) as raised:
            await db.fetch_all(query("SELECT * FROM secret_internal_table"))

        assert "secret_internal_table" not in raised.value.message
        assert "secret_internal_table" in raised.value.technical_message

    async def test_a_driver_error_while_streaming_becomes_a_database_error(
        self, db: Database
    ) -> None:
        with pytest.raises(DatabaseError):
            [row async for row in db.stream(query("SELECT * FROM table_that_does_not_exist"))]

    async def test_an_exhausted_pool_becomes_a_database_error(self, database_url: str) -> None:
        """A saturated pool must not leak `psycopg_pool.PoolTimeout` to the host application."""
        database = Database(AtlasConfig(database_url=database_url, pool_max_size=1))
        await database.open()
        try:
            async with database.connection():
                database.pool.timeout = 0.1

                with pytest.raises(DatabaseError):
                    await database.fetch_all(query("SELECT 1"))
        finally:
            await database.close()


class TestStream:
    async def test_it_yields_every_row_as_a_dict(self, db: Database) -> None:
        rows = [row async for row in db.stream(query("SELECT generate_series(1, 3) AS n"))]

        assert rows == [{"n": 1}, {"n": 2}, {"n": 3}]

    async def test_it_uses_a_named_server_side_cursor(
        self, db: Database, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        cursors = []
        original = psycopg.AsyncConnection.cursor

        def spy(self: psycopg.AsyncConnection, *args: object, **kwargs: object) -> object:
            cursor = original(self, *args, **kwargs)
            cursors.append(cursor)
            return cursor

        monkeypatch.setattr(psycopg.AsyncConnection, "cursor", spy)

        async for _ in db.stream(query("SELECT generate_series(1, 10) AS n"), batch_size=4):
            break

        assert [type(cursor).__name__ for cursor in cursors] == ["AsyncServerCursor"]
        assert cursors[0].itersize == 4

    async def test_it_does_not_materialise_the_whole_result(self, db: Database) -> None:
        many_rows = query("SELECT generate_series(1, 50000000) AS n")

        first = await anext(aiter(db.stream(many_rows, batch_size=10)))

        assert first == {"n": 1}

    async def test_the_connection_returns_to_the_pool_after_streaming(self, db: Database) -> None:
        async for _ in db.stream(query("SELECT generate_series(1, 1000) AS n")):
            break

        assert await db.fetch_one(query("SELECT 1 AS n")) == {"n": 1}
