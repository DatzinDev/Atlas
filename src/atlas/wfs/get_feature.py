"""Stream bounded GeoJSON features from a server-side PostGIS cursor."""

import json
from collections.abc import AsyncGenerator
from typing import Any, Literal, cast

from anyio import CancelScope
from fastapi.encoders import jsonable_encoder
from fastapi.responses import StreamingResponse
from starlette.types import Receive, Scope, Send

from atlas.context import AtlasContext
from atlas.core.crs import require_crs
from atlas.core.exceptions import InvalidFilter
from atlas.core.logging import OperationLog
from atlas.core.predicates import Predicate
from atlas.core.types import BBox
from atlas.layers.models import Layer
from atlas.layers.permissions import check_permission
from atlas.postgis.geometry import to_geojson_feature
from atlas.postgis.queries import build_feature_query, count_query
from atlas.wfs.filters import Filter, compile_query_filters
from atlas.wfs.pagination import resolve_pagination


class _FeatureResponse(StreamingResponse):
    def __init__(
        self, body: AsyncGenerator[str, None], rows: AsyncGenerator[dict[str, Any], None]
    ) -> None:
        super().__init__(body, media_type="application/geo+json")
        self.rows = rows

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        try:
            await super().__call__(scope, receive, send)
        finally:
            # Sending headers may fail before the body iterator starts its own cleanup.
            with CancelScope(shield=True):
                await self.rows.aclose()


def parse_property_names(
    value: str | None, layer: Layer, *, allow_geometry: bool = False
) -> list[str] | None:
    if value is None or not value.strip():
        return None
    names = list(dict.fromkeys(name.strip() for name in value.split(",")))
    for name in names:
        if name not in layer.attribute_names and not (
            allow_geometry and name == layer.geometry_column
        ):
            raise InvalidFilter("unknown attribute", column=name)
    return names


def parse_sort_by(value: str | None, layer: Layer) -> list[tuple[str, Literal["ASC", "DESC"]]]:
    if value is None:
        return []
    ordering: list[tuple[str, Literal["ASC", "DESC"]]] = []
    seen = set()
    for item in value.split(","):
        parts = item.split()
        if not 1 <= len(parts) <= 2:
            raise InvalidFilter("sortBy requires a field and optional A or D direction")
        column = parts[0]
        direction = parts[1] if len(parts) == 2 else "A"
        if column not in layer.attribute_names or column == layer.geometry_column:
            raise InvalidFilter("unknown sort attribute", column=column)
        if direction not in ("A", "D") or column in seen:
            raise InvalidFilter("invalid or duplicate sort criterion")
        seen.add(column)
        ordering.append((column, "ASC" if direction == "A" else "DESC"))
    return ordering


async def get_feature(
    ctx: AtlasContext,
    *,
    type_names: str,
    bbox: BBox | None = None,
    bbox_srid: int | None = None,
    output_srid: int = 4326,
    filters: list[Filter] | None = None,
    predicate: Predicate | None = None,
    order_by: list[tuple[str, Literal["ASC", "DESC"]]] | None = None,
    property_names: list[str] | None = None,
    include_geometry: bool = True,
    count: int | None = None,
    start_index: int = 0,
    principal: Any = None,
    report: OperationLog | None = None,
) -> StreamingResponse:
    layer = ctx.registry.require_service(type_names, "wfs")
    await check_permission(ctx.permission_checker, layer, "query", principal)
    columns = property_names
    if property_names is not None:
        allowed = {*layer.attribute_names, layer.geometry_column}
        for name in property_names:
            if name not in allowed:
                raise InvalidFilter("unknown attribute", column=name)
        columns = [name for name in property_names if name != layer.geometry_column]
        include_geometry = include_geometry and layer.geometry_column in property_names
    require_crs(output_srid)
    page = resolve_pagination(ctx.config, count, start_index)
    where, params = compile_query_filters(
        layer,
        filters=filters,
        predicate=predicate,
        bbox=bbox,
        bbox_srid=bbox_srid if bbox_srid is not None else ctx.config.default_srid,
    )
    query, values = build_feature_query(
        layer.table_info,
        columns=columns,
        where=where,
        params=params,
        target_srid=output_srid,
        geometry_as="geojson" if include_geometry else "none",
        limit=page.limit,
        offset=page.offset,
        order_by=order_by or (),
    )
    count_sql, count_values = count_query(layer.table_info, where, params)
    matched = await ctx.db.fetch_one(count_sql, count_values)
    rows = cast(AsyncGenerator[dict[str, Any], None], ctx.db.stream(query, values))

    def encode(row: dict[str, Any]) -> str:
        return json.dumps(
            jsonable_encoder(to_geojson_feature(row, layer.id_column, layer.geometry_column)),
            ensure_ascii=False,
            allow_nan=False,
        )

    try:
        first = await anext(rows, None)
        first_json = encode(first) if first is not None else None
    except BaseException:
        await rows.aclose()
        raise

    async def body() -> AsyncGenerator[str, None]:
        returned = 0
        try:
            yield '{"type":"FeatureCollection","features":['
            if first_json is not None:
                yield first_json
                returned += 1
                if report is not None:
                    report.feature_count = returned
            async for row in rows:
                yield ("," if returned else "") + encode(row)
                returned += 1
                if report is not None:
                    report.feature_count = returned
            metadata = {
                "numberMatched": matched["count"] if matched is not None else 0,
                "numberReturned": returned,
                "startIndex": page.offset,
                "crs": {"type": "name", "properties": {"name": f"EPSG:{output_srid}"}},
            }
            if report is not None:
                report.feature_count = returned
            yield "]," + json.dumps(metadata)[1:]
        finally:
            with CancelScope(shield=True):
                await rows.aclose()

    return _FeatureResponse(body(), rows)
