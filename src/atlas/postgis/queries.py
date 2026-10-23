"""Compose feature queries with catalog identifiers and bound values only."""

import math
from collections.abc import AsyncGenerator, Sequence
from dataclasses import dataclass
from typing import Any, Literal, cast

from anyio import CancelScope
from psycopg import sql
from shapely.geometry.base import BaseGeometry  # type: ignore[import-untyped]
from shapely.wkb import loads  # type: ignore[import-untyped]

from atlas.core.crs import parse_crs, require_crs
from atlas.core.exceptions import InvalidFilter, InvalidGeometry, QueryTooLarge
from atlas.core.types import BBox
from atlas.postgis.connection import Database
from atlas.postgis.introspection import TableInfo

FILTER_OPERATORS = {
    "eq": "=",
    "ne": "!=",
    "gt": ">",
    "gte": ">=",
    "lt": "<",
    "lte": "<=",
    "in": "IN",
    "like": "LIKE",
}


@dataclass
class RenderBudget:
    features: int
    bytes: int


def _validate_bbox(bbox: BBox) -> None:
    if (
        len(bbox) != 4
        or not all(math.isfinite(v) for v in bbox)
        or not (bbox[0] < bbox[2] and bbox[1] < bbox[3])
    ):
        raise InvalidGeometry("bbox must contain finite ordered minx,miny,maxx,maxy")


def parse_bbox(value: str) -> tuple[BBox, int | None]:
    parts = value.split(",")
    if len(parts) not in (4, 5):
        raise InvalidGeometry("bbox requires four coordinates and an optional CRS")
    try:
        bbox = (float(parts[0]), float(parts[1]), float(parts[2]), float(parts[3]))
    except ValueError as exc:
        raise InvalidGeometry("bbox coordinates must be numbers") from exc
    _validate_bbox(bbox)
    return bbox, parse_crs(parts[4]) if len(parts) == 5 else None


def bbox_predicate(info: TableInfo, bbox: BBox, bbox_srid: int) -> tuple[sql.Composed, list[Any]]:
    _validate_bbox(bbox)
    require_crs(bbox_srid)
    envelope: sql.Composable = sql.SQL("ST_MakeEnvelope(%s,%s,%s,%s,%s)")
    params: list[Any] = [*bbox, bbox_srid]
    if bbox_srid != info.srid:
        envelope = sql.SQL("ST_Transform({}, %s)").format(envelope)
        params.append(info.srid)
    return sql.SQL("{} && {}").format(sql.Identifier(info.geometry_column), envelope), params


def attribute_predicate(
    info: TableInfo, column: str, operator: str, value: Any
) -> tuple[sql.Composed, list[Any]]:
    if column not in {c.name for c in info.columns} or column == info.geometry_column:
        raise InvalidFilter("unknown attribute", column=column)
    if operator not in FILTER_OPERATORS:
        raise InvalidFilter("unknown filter operator", operator=operator)
    if operator == "in":
        if not isinstance(value, list) or not value:
            raise InvalidFilter("IN requires a non-empty list")
        predicate = sql.SQL("{} IN ({})").format(
            sql.Identifier(column), sql.SQL(", ").join(sql.Placeholder() for _ in value)
        )
        return predicate, value
    return sql.SQL("{} {} %s").format(
        sql.Identifier(column), sql.SQL(FILTER_OPERATORS[operator])
    ), [value]


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
    order_by: Sequence[tuple[str, Literal["ASC", "DESC"]]] = (),
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
    ordering = list(order_by)
    if info.primary_key is not None and not any(name == info.primary_key for name, _ in ordering):
        ordering.append((info.primary_key, "ASC"))
    directions = {"ASC": sql.SQL("ASC"), "DESC": sql.SQL("DESC")}
    order_fields = []
    seen = set()
    for name, direction in ordering:
        if name not in whitelist or name == info.geometry_column:
            raise InvalidFilter("unknown order attribute", column=name)
        if direction not in directions or name in seen:
            raise InvalidFilter("invalid or duplicate order criterion", column=name)
        seen.add(name)
        order_fields.append(sql.SQL("{} {}").format(sql.Identifier(name), directions[direction]))
    if order_fields:
        query += sql.SQL(" ORDER BY {}").format(sql.SQL(", ").join(order_fields))
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


async def fetch_render_geometries(
    db: Database,
    info: TableInfo,
    *,
    bbox: BBox,
    bbox_srid: int,
    target_srid: int,
    where: Sequence[sql.Composed] = (),
    params: Sequence[Any] = (),
    limit: int | None = None,
    budget: RenderBudget | None = None,
) -> list[BaseGeometry]:
    """Return the complete window or reject it before rendering a partial map."""
    require_crs(target_srid)
    predicate, bbox_params = bbox_predicate(info, bbox, bbox_srid)
    if limit is not None and limit <= 0:
        raise InvalidFilter("render limit must be positive")
    budget = budget or RenderBudget(db._config.max_render_features, db._config.max_render_bytes)
    cap = min(limit or db._config.max_render_features, budget.features)
    query, values = build_feature_query(
        info,
        columns=[],
        where=[*where, predicate],
        params=[*params, *bbox_params],
        geometry_as="wkb",
        target_srid=target_srid,
        limit=cap + 1,
    )
    # Bound the whole cursor's payload in SQL, so batching cannot multiply the WKB budget.
    query = sql.SQL(
        "SELECT size, CASE WHEN used <= %s THEN data END AS data FROM ("
        "SELECT {geom} AS data, octet_length({geom}) AS size, "
        "sum(octet_length({geom})) OVER (ROWS UNBOUNDED PRECEDING) AS used "
        "FROM ({query}) AS render_rows) AS bounded_rows"
    ).format(geom=sql.Identifier(info.geometry_column), query=query)
    rows = cast(
        AsyncGenerator[dict[str, Any], None],
        db.stream(query, [budget.bytes, *values]),
    )
    geometries: list[BaseGeometry] = []
    try:
        async for row in rows:
            size = row["size"]
            if size is None:
                continue
            if len(geometries) >= cap or size > budget.bytes:
                raise QueryTooLarge(
                    "the complete map exceeds the render budget; use a smaller window"
                )
            budget.features -= 1
            budget.bytes -= size
            geometries.append(loads(bytes(row["data"])))
    finally:
        with CancelScope(shield=True):
            await rows.aclose()
    return geometries


def intersects_bbox_predicate(
    info: TableInfo,
    bbox: BBox,
    bbox_srid: int,
) -> tuple[sql.Composed, list[Any]]:
    """Combine the index-friendly candidate filter with an exact window intersection."""
    candidate, values = bbox_predicate(info, bbox, bbox_srid)
    envelope = sql.SQL("ST_Transform(ST_MakeEnvelope(%s,%s,%s,%s,%s), %s)")
    exact = sql.SQL("ST_Intersects({}, {})").format(sql.Identifier(info.geometry_column), envelope)
    return sql.SQL("({}) AND ({})").format(candidate, exact), [*values, *bbox, bbox_srid, info.srid]
