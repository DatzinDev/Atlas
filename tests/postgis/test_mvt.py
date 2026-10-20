"""Decode real PostGIS tiles, including properties, clipping and empty tiles."""

import mapbox_vector_tile
import pytest
from psycopg import sql

from atlas.core.exceptions import InvalidParameter
from atlas.core.predicates import Comparison
from atlas.core.tiles import TileCoord
from atlas.postgis.introspection import inspect_table
from atlas.postgis.mvt import build_mvt_query


async def test_native_layers_and_empty(db):
    for name in ("municipios", "carreteras", "escuelas"):
        info = await inspect_table(db, "geo", name)
        query, values = build_mvt_query(
            info, TileCoord(z=6, x=13, y=28), layer_name=name, candidate_limit=100
        )
        row = await db.fetch_one(query, values)
        layer = mapbox_vector_tile.decode(bytes(row["tile"]))[name]
        assert row["count"] == len(layer["features"]) > 0
        assert layer["extent"] == 4096
        assert all(f["id"] == f["properties"]["id"] for f in layer["features"])
        query, values = build_mvt_query(
            info, TileCoord(z=6, x=0, y=0), layer_name=name, candidate_limit=100
        )
        row = await db.fetch_one(query, values)
        assert row["count"] == 0 and bytes(row["tile"]) == b""


async def test_whitelist_filter_and_cap_signal(db, municipios_layer):
    info = municipios_layer.table_info
    for properties in ([], ["geom"], ["id", "id"], ["id; DROP TABLE geo.municipios"]):
        with pytest.raises(InvalidParameter):
            build_mvt_query(
                info,
                TileCoord(z=0, x=0, y=0),
                layer_name="safe",
                properties=properties,
                candidate_limit=10,
            )
    query, values = build_mvt_query(
        info,
        TileCoord(z=6, x=13, y=28),
        layer_name="bound'name",
        properties=["nombre"],
        predicate=Comparison(op="eq", field="id", value=1),
        candidate_limit=1,
    )
    assert "bound'name" not in query.as_string()
    row = await db.fetch_one(query, values)
    features = mapbox_vector_tile.decode(bytes(row["tile"]))["bound'name"]["features"]
    assert len(features) == 1 and features[0]["id"] == 1
    assert features[0]["properties"] == {"nombre": "San Bernardo"}
    query, values = build_mvt_query(
        info, TileCoord(z=6, x=13, y=28), layer_name="safe", candidate_limit=1
    )
    row = await db.fetch_one(query, values)
    assert row["count"] == 2 and bytes(row["tile"]) == b""


async def test_property_precision_ids_and_buffer_clipping(db):
    async with db.connection() as conn:
        await conn.execute(
            sql.SQL("""CREATE TABLE geo.mvt_fixture (
            id bigint PRIMARY KEY, amount numeric, day date, payload jsonb,
            geom geometry(Point,3857));
            INSERT INTO geo.mvt_fixture VALUES
            (9007199254740992,12345678901234567890.123456789,'2026-10-20','{}',ST_SetSRID(ST_Point(-10,10),3857)),
            (-1,NULL,NULL,NULL,ST_SetSRID(ST_Point(10,10),3857));""")
        )
    try:
        info = await inspect_table(db, "geo", "mvt_fixture")
        for buffer, expected in ((0, 1), (64, 2)):
            query, values = build_mvt_query(
                info, TileCoord(z=1, x=0, y=0), layer_name="test", buffer=buffer, candidate_limit=10
            )
            row = await db.fetch_one(query, values)
            features = mapbox_vector_tile.decode(bytes(row["tile"]))["test"]["features"]
            assert len(features) == expected
            first = next(f for f in features if f["properties"]["id"] > 0)
            assert first["properties"]["amount"] == "12345678901234567890.123456789"
            assert first["properties"]["day"] == "2026-10-20"
            assert "payload" not in first["properties"]
            assert first["id"] == 0  # decoder default for an absent protobuf ID
        with pytest.raises(InvalidParameter):
            build_mvt_query(
                info,
                TileCoord(z=0, x=0, y=0),
                layer_name="test",
                properties=["payload"],
                candidate_limit=1,
            )
    finally:
        async with db.connection() as conn:
            await conn.execute(sql.SQL("DROP TABLE geo.mvt_fixture"))
