"""Create a separate disposable database even when CI shares a PostGIS server."""

from uuid import uuid4

import psycopg
import pytest
from psycopg import sql
from psycopg.conninfo import make_conninfo

from atlas.core.config import AtlasConfig
from atlas.postgis.connection import Database


@pytest.fixture
async def bench_db(database_url):
    name = "atlas_bench_test_" + uuid4().hex
    admin = await psycopg.AsyncConnection.connect(database_url, autocommit=True)
    db = Database(AtlasConfig(database_url=make_conninfo(database_url, dbname=name)))
    try:
        await admin.execute(sql.SQL("CREATE DATABASE {}").format(sql.Identifier(name)))
        await db.open()
        async with db.connection() as conn:
            await conn.execute(sql.SQL("CREATE EXTENSION postgis"))
        yield db
    finally:
        await db.close()
        await admin.execute(sql.SQL("DROP DATABASE IF EXISTS {}").format(sql.Identifier(name)))
        await admin.close()
