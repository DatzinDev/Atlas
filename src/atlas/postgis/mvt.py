"""Compose native PostGIS MVT without materializing geometries in Python."""

from collections.abc import Sequence
from typing import Any

from psycopg import sql

from atlas.core.exceptions import InvalidParameter
from atlas.core.predicates import Predicate
from atlas.core.tiles import TileCoord, tile_bounds
from atlas.postgis.introspection import TableInfo
from atlas.postgis.predicates import compile_predicate
from atlas.postgis.queries import bbox_predicate

_DIRECT = {"text", "varchar", "bpchar", "bool", "int2", "int4", "int8"}
_TEXT = {"numeric", "decimal", "date", "timestamp", "timestamptz"}
_FLOAT = {"float4", "float8"}


def build_mvt_query(
    info: TableInfo,
    tile: TileCoord,
    *,
    layer_name: str,
    properties: Sequence[str] | None = None,
    predicate: Predicate | None = None,
    extent: int = 4096,
    buffer: int = 64,
    candidate_limit: int,
) -> tuple[sql.Composed, list[Any]]:
    if (
        type(extent) is not int
        or not 1 <= extent <= 65536
        or type(buffer) is not int
        or not 0 <= buffer <= extent
        or type(candidate_limit) is not int
        or candidate_limit < 1
    ):
        raise InvalidParameter("invalid MVT budgets")
    bounds = tile_bounds(tile, max_zoom=30)
    columns = {c.name: c.data_type for c in info.columns if c.name != info.geometry_column}
    names = (
        list(properties)
        if properties is not None
        else [name for name, kind in columns.items() if kind in _DIRECT | _TEXT | _FLOAT]
    )
    if (properties is not None and not names) or len(set(names)) != len(names):
        raise InvalidParameter("properties must be nonempty and unique")
    expressions: list[sql.Composable] = []
    for name in names:
        kind = columns.get(name)
        column = sql.Identifier(name)
        if kind in _DIRECT:
            expression: sql.Composable = column
        elif kind in _TEXT:
            expression = sql.SQL("{}::text").format(column)
        elif kind in _FLOAT:
            expression = sql.SQL(
                "CASE WHEN {c}::text IN ('NaN','Infinity','-Infinity') THEN NULL ELSE {c} END"
            ).format(c=column)
        else:
            raise InvalidParameter("unsupported or unknown MVT property", column=name)
        expressions.append(sql.SQL("{} AS {}").format(expression, column))
    # Internal names cannot shadow published attributes, including explicitly selected names.
    geom_name = "__atlas_geom"
    while geom_name in columns:
        geom_name += "_"
    id_name = geom_name + "_id"
    while id_name in columns:
        id_name += "_"
    geom = sql.Identifier(info.geometry_column)
    expressions.append(sql.SQL("{} AS {}").format(geom, sql.Identifier(geom_name)))
    if info.primary_key in columns and columns[info.primary_key] in {"int2", "int4", "int8"}:
        expressions.append(
            sql.SQL(
                "CASE WHEN {pk} BETWEEN 0 AND 9007199254740991 THEN {pk} ELSE NULL END AS {id}"
            ).format(pk=sql.Identifier(info.primary_key), id=sql.Identifier(id_name))
        )
        feature_id: str | None = id_name
    else:
        feature_id = None
    margin = (bounds[2] - bounds[0]) * buffer / extent
    expanded = (bounds[0] - margin, bounds[1] - margin, bounds[2] + margin, bounds[3] + margin)
    spatial, spatial_values = bbox_predicate(info, expanded, 3857)
    selection, selection_values = compile_predicate(predicate, info)
    where = [spatial, sql.SQL("{} IS NOT NULL AND NOT ST_IsEmpty({})").format(geom, geom)]
    if predicate is not None:
        where.append(selection)
    projected = sql.SQL(
        "ST_AsMVTGeom(ST_Transform({},3857), ST_MakeEnvelope(%s,%s,%s,%s,3857), %s, %s, true) AS {}"
    ).format(sql.Identifier(geom_name), sql.Identifier(geom_name))
    output = [sql.Identifier(name) for name in names]
    if feature_id:
        output.append(sql.Identifier(feature_id))
    query = sql.SQL("""
        WITH candidates AS MATERIALIZED (
            SELECT {columns} FROM {table} WHERE {where} LIMIT %s
        ), total AS (SELECT count(*) AS count FROM candidates)
        SELECT count, CASE WHEN count <= %s THEN (
            SELECT ST_AsMVT(encoded, %s, %s, %s, %s) FROM (
                SELECT {output} {projected} FROM candidates
            ) encoded WHERE {geom} IS NOT NULL
        ) ELSE ''::bytea END AS tile FROM total
    """).format(
        columns=sql.SQL(", ").join(expressions),
        table=sql.Identifier(info.schema_name, info.table_name),
        where=sql.SQL(" AND ").join(where),
        output=sql.SQL("").join([sql.SQL(", ").join(output), sql.SQL(", ")])
        if output
        else sql.SQL(""),
        projected=projected,
        geom=sql.Identifier(geom_name),
    )
    return query, [
        *spatial_values,
        *selection_values,
        candidate_limit + 1,
        candidate_limit,
        layer_name,
        extent,
        geom_name,
        feature_id,
        *bounds,
        extent,
        buffer,
    ]
