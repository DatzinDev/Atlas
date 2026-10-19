"""The pool every Atlas query goes through, and the only place psycopg errors are translated."""

from __future__ import annotations

from collections.abc import AsyncIterator, Iterator, Sequence
from contextlib import asynccontextmanager, contextmanager
from typing import Any
from uuid import uuid4

import psycopg
from psycopg import AsyncConnection, sql
from psycopg.rows import dict_row
from psycopg_pool import AsyncConnectionPool

from atlas.core.config import AtlasConfig
from atlas.core.exceptions import DatabaseError, QueryTimeout


class Database:
    """An async connection pool over a single PostGIS database.

    Atlas owns the pool: it is opened on startup, closed on shutdown, and every connection it hands
    out already carries the row factory and the `statement_timeout` derived from `AtlasConfig`.
    No psycopg exception escapes this class — callers only ever see `QueryTimeout` or
    `DatabaseError`.
    """

    def __init__(self, config: AtlasConfig) -> None:
        self._config = config
        self._pool: AsyncConnectionPool[AsyncConnection[dict[str, Any]]] | None = None

    @property
    def pool(self) -> AsyncConnectionPool[AsyncConnection[dict[str, Any]]] | None:
        """The live pool, or `None` while the database is closed."""
        return self._pool

    async def open(self) -> None:
        """Open the pool and wait until it can serve connections. A second call does nothing."""
        if self._pool is not None:
            return
        pool: AsyncConnectionPool[AsyncConnection[dict[str, Any]]] = AsyncConnectionPool(
            conninfo=self._config.database_url,
            min_size=self._config.pool_min_size,
            max_size=self._config.pool_max_size,
            kwargs={"row_factory": dict_row},
            configure=self._configure,
            open=False,
        )
        with _as_atlas_error():
            await pool.open(wait=True)
        self._pool = pool

    async def close(self) -> None:
        """Close the pool. Safe to call on an already closed — or never opened — database."""
        if self._pool is None:
            return
        pool, self._pool = self._pool, None
        await pool.close()

    async def _configure(self, conn: AsyncConnection[dict[str, Any]]) -> None:
        """Apply the configured query timeout to a connection the pool has just created.

        The `SET` runs in autocommit because the pool requires `configure` to hand the connection
        back outside a transaction: one left in `INTRANS` is discarded and never joins the pool.
        `SET` is transactional in PostgreSQL, so autocommit is also what makes it stick for the
        session instead of dying with the next rollback.
        """
        timeout_ms = int(self._config.query_timeout * 1000)
        await conn.set_autocommit(True)
        await conn.execute(sql.SQL("SET statement_timeout = {}").format(sql.Literal(timeout_ms)))
        await conn.set_autocommit(False)

    @asynccontextmanager
    async def connection(self) -> AsyncIterator[AsyncConnection[dict[str, Any]]]:
        """Borrow a connection from the pool for the duration of the block.

        The transaction is committed on a clean exit and rolled back on error, and any psycopg
        failure raised inside the block leaves as an `AtlasError`.
        """
        if self._pool is None:
            raise DatabaseError("the connection pool is not open; call Database.open() first")
        with _as_atlas_error():
            async with self._pool.connection() as conn:
                yield conn

    async def fetch_all(
        self, query: sql.Composed, params: Sequence[Any] | None = None
    ) -> list[dict[str, Any]]:
        """Run a query and return every row. Only for results already bounded by a `LIMIT`."""
        async with self.connection() as conn, conn.cursor() as cur:
            await cur.execute(query, params)
            return await cur.fetchall()

    async def fetch_one(
        self, query: sql.Composed, params: Sequence[Any] | None = None
    ) -> dict[str, Any] | None:
        """Run a query and return its first row, or `None` when it matched nothing."""
        async with self.connection() as conn, conn.cursor() as cur:
            await cur.execute(query, params)
            return await cur.fetchone()

    async def stream(
        self,
        query: sql.Composed,
        params: Sequence[Any] | None = None,
        batch_size: int = 1000,
    ) -> AsyncIterator[dict[str, Any]]:
        """Yield rows through a server-side cursor, fetching `batch_size` of them at a time.

        This is how downloads read their data: the result set stays in PostgreSQL and never lands
        in memory as a whole, however many features the query matches.
        """
        async with (
            self.connection() as conn,
            conn.cursor(name=f"atlas_stream_{uuid4().hex}") as cur,
        ):
            cur.itersize = batch_size
            await cur.execute(query, params)
            async for row in cur:
                yield row


@contextmanager
def _as_atlas_error() -> Iterator[None]:
    """Translate psycopg failures into the Atlas taxonomy.

    A cancelled query is the `statement_timeout` firing; everything else is reported as a generic
    `DatabaseError`, which keeps the driver's message on `technical_message` and out of the
    response.
    """
    try:
        yield
    except psycopg.errors.QueryCanceled as exc:
        raise QueryTimeout("the query took longer than the configured timeout") from exc
    except psycopg.Error as exc:
        raise DatabaseError(str(exc).strip()) from exc
