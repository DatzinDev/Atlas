"""Compose feature queries with catalog identifiers and bound values only."""

from collections.abc import Sequence
from typing import Any, Literal

from psycopg import sql

from atlas.core.exceptions import InvalidFilter
from atlas.postgis.introspection import TableInfo


def _where_sql(where: Sequence[sql.Composed]) -> sql.Composed:
    return sql.SQL(" WHERE {}").format(sql.SQL(" AND ").join(where)) if where else sql.Composed([])


def build_feature_query(
    info: TableInfo,
    *,
    columns: Sequence[str] | None = None,
    where: Sequence[sql.Composed] = (),
    params: Sequence[Any] = (),
    geometry_as: Literal["geojson", "wkb", "none"] = "geojson",
    target_srid: int | None = None,
    limit: int | None = None,
    offset: int = 0,
) -> tuple[sql.Composed, list[Any]]:
    names = [c.name for c in info.columns] if columns is None else list(columns)
    if info.primary_key is not None and info.primary_key not in names:
        names.append(info.primary_key)
    whitelist = {c.name for c in info.columns}
    for name in names:
        if name not in whitelist or name == info.geometry_column:
            raise InvalidFilter("unknown attribute", column=name)
    if geometry_as not in ("geojson", "wkb", "none"):
        raise InvalidFilter("unknown geometry representation")
    if offset < 0 or (limit is not None and limit < 0):
        raise InvalidFilter("limit and offset cannot be negative")
    fields: list[sql.Composable] = [sql.Identifier(name) for name in dict.fromkeys(names)]
    values: list[Any] = []
    geom: sql.Composable = sql.Identifier(info.geometry_column)
    if geometry_as != "none":
        if target_srid is not None:
            geom = sql.SQL("ST_Transform({}, %s)").format(geom)
            values.append(target_srid)
        expression = sql.SQL(
            "ST_AsGeoJSON({})::json" if geometry_as == "geojson" else "ST_AsBinary({})"
        )
        fields.append(
            sql.SQL("{} AS {}").format(
                expression.format(geom), sql.Identifier(info.geometry_column)
            )
        )
    query = sql.SQL("SELECT {} FROM {}{}").format(
        sql.SQL(", ").join(fields),
        sql.Identifier(info.schema_name, info.table_name),
        _where_sql(where),
    )
    values.extend(params)
    if limit is not None:
        query += sql.SQL(" LIMIT %s")
        values.append(limit)
    query += sql.SQL(" OFFSET %s")
    values.append(offset)
    return query, values


def count_query(
    info: TableInfo, where: Sequence[sql.Composed], params: Sequence[Any]
) -> tuple[sql.Composed, list[Any]]:
    return sql.SQL("SELECT count(*) AS count FROM {}{}").format(
        sql.Identifier(info.schema_name, info.table_name), _where_sql(where)
    ), list(params)
