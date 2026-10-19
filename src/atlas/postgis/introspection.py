"""Discover spatial relation metadata without reading its dataset into memory."""

from typing import Literal, cast, get_args

from psycopg import sql
from pydantic import BaseModel

from atlas.core.exceptions import InvalidGeometry, InvalidParameter, LayerNotFound
from atlas.postgis.connection import Database

GeometryType = Literal[
    "Point",
    "LineString",
    "Polygon",
    "MultiPoint",
    "MultiLineString",
    "MultiPolygon",
    "GeometryCollection",
    "Geometry",
]
_GEOMETRY_TYPES = {name.upper(): name for name in get_args(GeometryType)}


class ColumnInfo(BaseModel):
    name: str
    data_type: str
    nullable: bool


class TableInfo(BaseModel):
    schema_name: str
    table_name: str
    geometry_column: str
    geometry_type: GeometryType
    srid: int
    columns: list[ColumnInfo]
    primary_key: str | None
    is_view: bool


def parse_source(source: str, default_schema: str = "public") -> tuple[str, str]:
    """Parse an unquoted table name or schema.table pair, preserving identifier case."""
    parts = source.split(".")
    if len(parts) == 1:
        parts.insert(0, default_schema)
    if len(parts) != 2 or any(not part.strip() or "\x00" in part for part in parts):
        raise InvalidParameter("source must be a table name or schema.table pair")
    return parts[0], parts[1]


async def inspect_table(db: Database, schema: str, table: str) -> TableInfo:
    """Inspect a table or view; the first geometry column in declaration order wins.

    All geometry columns are excluded from attributes. Generic geometry metadata is inferred
    from the first non-null row; an empty generic relation cannot be inferred and is rejected.
    Composite primary keys have no single feature identifier and are reported as None.
    """
    relation = await db.fetch_one(
        sql.SQL("""
            SELECT c.oid, c.relkind IN ('v', 'm') AS is_view
            FROM pg_catalog.pg_class c
            JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = %s AND c.relname = %s AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
        """).format(),
        [schema, table],
    )
    if relation is None:
        raise LayerNotFound("the requested table or view does not exist")
    attributes = await db.fetch_all(
        sql.SQL("""
            SELECT a.attname AS name, t.typname AS data_type, NOT a.attnotnull AS nullable,
                   g.type AS geometry_type, g.srid,
                   EXISTS (
                       SELECT 1 FROM pg_catalog.pg_index i
                       WHERE i.indrelid = a.attrelid AND i.indisprimary
                         AND i.indnkeyatts = 1 AND a.attnum = i.indkey[0]
                   ) AS primary_key
            FROM pg_catalog.pg_attribute a
            JOIN pg_catalog.pg_type t ON t.oid = a.atttypid
            LEFT JOIN geometry_columns g ON g.f_table_schema = %s AND g.f_table_name = %s
                                        AND g.f_geometry_column = a.attname
            WHERE a.attrelid = %s AND a.attnum > 0 AND NOT a.attisdropped
            ORDER BY a.attnum
        """).format(),
        [schema, table, relation["oid"]],
    )
    geometry = next((a for a in attributes if a["data_type"] == "geometry"), None)
    if geometry is None:
        raise InvalidGeometry("the relation has no geometry column")
    geometry_type, srid = geometry["geometry_type"], geometry["srid"]
    if geometry_type in (None, "GEOMETRY") or not srid:
        sample = await db.fetch_one(
            sql.SQL("""
                SELECT GeometryType({column}) AS geometry_type, ST_SRID({column}) AS srid
                FROM {relation} WHERE {column} IS NOT NULL LIMIT 1
            """).format(
                column=sql.Identifier(geometry["name"]), relation=sql.Identifier(schema, table)
            ),
        )
        if sample is None:
            raise InvalidGeometry("cannot infer geometry metadata from an empty geometry column")
        geometry_type, srid = sample["geometry_type"], sample["srid"]
    normalized_type = _GEOMETRY_TYPES.get(geometry_type)
    if normalized_type is None:
        raise InvalidGeometry("the geometry type is not supported")
    return TableInfo(
        schema_name=schema,
        table_name=table,
        geometry_column=geometry["name"],
        geometry_type=cast(GeometryType, normalized_type),
        srid=srid,
        columns=[ColumnInfo(**a) for a in attributes if a["data_type"] != "geometry"],
        primary_key=next((a["name"] for a in attributes if a["primary_key"]), None),
        is_view=relation["is_view"],
    )
