"""Deterministic vector fixtures restricted to disposable benchmark databases."""

import math
from contextlib import asynccontextmanager
from uuid import uuid4

from anyio import CancelScope
from psycopg import sql

from atlas.core.tiles import TileCoord, tile_bounds
from atlas.layers.metadata import build_layer
from atlas.layers.models import LayerServices
from atlas.postgis.connection import Database

SELECTIVE = TileCoord(z=10, x=216, y=456)
GLOBAL = TileCoord(z=0, x=0, y=0)
MOSAIC = [TileCoord(z=7, x=x, y=y) for x in range(24, 28) for y in range(56, 60)]


@asynccontextmanager
async def dataset(db: Database, rows: int, kind: str):
    """Create and remove only this run's schema; never reuse application relations."""
    if type(rows) is not int or not 1 <= rows <= 1_000_000:
        raise ValueError("rows must be 1..1000000")
    geometry = {
        "point": sql.SQL("ST_SetSRID(ST_Point(x,y),3857)"),
        "line": sql.SQL("ST_SetSRID(ST_MakeLine(ST_Point(x,y), ST_Point(x+%s,y-%s)),3857)"),
        "polygon": sql.SQL("ST_MakeEnvelope(x,y-%s,x+%s,y,3857)"),
    }
    if kind not in geometry:
        raise ValueError("unknown geometry kind")
    database = await db.fetch_one(sql.SQL("SELECT current_database() AS name").format())
    if database is None or not database["name"].startswith("atlas_bench_"):
        raise ValueError("refusing DDL outside an atlas_bench_* disposable database")
    schema = "run_" + uuid4().hex
    relation = sql.Identifier(schema, "features")
    side = math.ceil(math.sqrt(rows))
    left, _, right, top = tile_bounds(TileCoord(z=5, x=6, y=14))
    step = (right - left) / side
    async with db.connection() as conn:
        await conn.execute(sql.SQL("CREATE SCHEMA {}").format(sql.Identifier(schema)))
    try:
        async with db.connection() as conn:
            await conn.execute(
                sql.SQL("""
                    CREATE TABLE {table} AS WITH grid AS (
                        SELECT i::bigint AS id, %s+((i-1) %% %s)*%s AS x,
                               %s-((i-1)/%s)*%s AS y FROM generate_series(1,%s) i
                    ) SELECT id, {geometry} AS geom FROM grid
                """).format(table=relation, geometry=geometry[kind]),
                [left + step * 0.2, side, step, top - step * 0.2, side, step, rows]
                + ([step * 0.4, step * 0.4] if kind != "point" else []),
            )
            await conn.execute(sql.SQL("ALTER TABLE {} ADD PRIMARY KEY (id)").format(relation))
            await conn.execute(sql.SQL("CREATE INDEX ON {} USING GIST (geom)").format(relation))
            await conn.execute(sql.SQL("ANALYZE {}").format(relation))
        yield await build_layer(
            db, schema + ".features", name=kind, services=LayerServices(mvt=True)
        )
    finally:
        with CancelScope(shield=True):
            async with db.connection() as conn:
                await conn.execute(sql.SQL("DROP SCHEMA {} CASCADE").format(sql.Identifier(schema)))
