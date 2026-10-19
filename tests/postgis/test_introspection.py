"""Spatial catalog discovery against real PostGIS, including untyped view geometries."""

from collections.abc import AsyncIterator

import pytest
from psycopg import sql

from atlas.core.exceptions import InvalidGeometry, InvalidParameter, LayerNotFound
from atlas.postgis.connection import Database
from atlas.postgis.introspection import inspect_table, parse_source


@pytest.mark.parametrize(
    ("source", "default", "expected"),
    [
        ("municipios", "public", ("public", "municipios")),
        ("municipios", "geo", ("geo", "municipios")),
        ("geo.municipios", "public", ("geo", "municipios")),
    ],
)
def test_parse_source(source: str, default: str, expected: tuple[str, str]) -> None:
    assert parse_source(source, default) == expected


@pytest.mark.parametrize("source", ["", ".table", "schema.", "a.b.c", "a. ", "bad\x00"])
def test_parse_source_rejects_invalid_names(source: str) -> None:
    with pytest.raises(InvalidParameter):
        parse_source(source)


@pytest.fixture
async def spatial_cases(db: Database) -> AsyncIterator[Database]:
    async with db.connection() as conn:
        await conn.execute("""
            CREATE TABLE geo.introspection_cases (
                id integer PRIMARY KEY, label text, first_geom geometry(Point, 4326),
                second_geom geometry(Polygon, 4326)
            );
            INSERT INTO geo.introspection_cases (id, first_geom)
            VALUES (1, ST_SetSRID(ST_MakePoint(1, 2), 4326));
            CREATE VIEW geo.introspection_generic AS
            SELECT id, ST_Translate(first_geom, 0, 0)::geometry AS geom
            FROM geo.introspection_cases;
            CREATE TABLE geo.introspection_plain (id integer);
            CREATE TABLE geo.introspection_empty (geom geometry);
            CREATE TABLE geo.introspection_composite (
                a integer, b integer, geom geometry(Point, 4326), PRIMARY KEY (a, b)
            );
        """)
    try:
        yield db
    finally:
        async with db.connection() as conn:
            await conn.execute("""
                DROP VIEW geo.introspection_generic;
                DROP TABLE geo.introspection_cases, geo.introspection_plain,
                           geo.introspection_empty, geo.introspection_composite;
            """)


async def test_table_metadata(db: Database) -> None:
    info = await inspect_table(db, "geo", "municipios")
    assert (info.schema_name, info.table_name) == ("geo", "municipios")
    assert (info.geometry_column, info.geometry_type, info.srid) == ("geom", "Polygon", 4326)
    assert info.primary_key == "id"
    assert not info.is_view
    assert [(c.name, c.data_type, c.nullable) for c in info.columns] == [
        ("id", "int4", False),
        ("clave", "text", False),
        ("nombre", "text", False),
        ("poblacion", "int4", False),
    ]


async def test_view_without_primary_key(db: Database) -> None:
    info = await inspect_table(db, "geo", "v_municipios_grandes")
    assert info.is_view
    assert info.primary_key is None
    assert (info.geometry_type, info.srid) == ("Polygon", 4326)


async def test_other_srid(db: Database) -> None:
    info = await inspect_table(db, "geo", "escuelas")
    assert (info.geometry_type, info.srid) == ("Point", 32613)


async def test_generic_view_infers_metadata_from_a_row(spatial_cases: Database) -> None:
    info = await inspect_table(spatial_cases, "geo", "introspection_generic")
    assert info.is_view and info.primary_key is None
    assert (info.geometry_type, info.srid) == ("Point", 4326)


async def test_missing_geometry_columns_entry_uses_catalog_and_sample(
    spatial_cases: Database,
) -> None:
    # Shadow the metadata view on the pooled connection to exercise the missing-entry path.
    async with spatial_cases.connection() as conn:
        await conn.execute("""
            CREATE TEMP VIEW geometry_columns AS
            SELECT * FROM public.geometry_columns WHERE false
        """)
        try:
            # A one-connection pool would deadlock; use the same connection for catalog queries.
            class ConnectionDatabase:
                async def fetch_one(self, query, params=None):
                    return await (await conn.execute(query, params)).fetchone()

                async def fetch_all(self, query, params=None):
                    return await (await conn.execute(query, params)).fetchall()

            info = await inspect_table(ConnectionDatabase(), "geo", "introspection_generic")
            assert (info.geometry_type, info.srid) == ("Point", 4326)
        finally:
            await conn.execute("DROP VIEW pg_temp.geometry_columns")


async def test_first_geometry_wins_and_all_geometries_are_excluded(spatial_cases: Database) -> None:
    info = await inspect_table(spatial_cases, "geo", "introspection_cases")
    assert info.geometry_column == "first_geom"
    assert [c.name for c in info.columns] == ["id", "label"]
    assert info.columns[1].nullable


async def test_empty_typed_table_and_composite_key(spatial_cases: Database) -> None:
    info = await inspect_table(spatial_cases, "geo", "introspection_composite")
    assert (info.geometry_type, info.srid) == ("Point", 4326)
    assert info.primary_key is None


@pytest.mark.parametrize("table", ["introspection_plain", "introspection_empty"])
async def test_unusable_geometry_is_rejected(spatial_cases: Database, table: str) -> None:
    with pytest.raises(InvalidGeometry):
        await inspect_table(spatial_cases, "geo", table)


@pytest.mark.parametrize("table", ["does_not_exist", "municipios'; DROP TABLE geo.municipios;--"])
async def test_missing_and_malicious_names_are_not_sql(db: Database, table: str) -> None:
    with pytest.raises(LayerNotFound):
        await inspect_table(db, "geo", table)
    assert await db.fetch_one(sql.SQL("SELECT count(*) AS n FROM geo.municipios").format()) == {
        "n": 5
    }
