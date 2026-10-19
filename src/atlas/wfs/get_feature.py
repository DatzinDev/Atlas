"""Stream bounded GeoJSON features from a server-side PostGIS cursor."""

import json
from collections.abc import AsyncGenerator
from typing import Any, cast

from fastapi.encoders import jsonable_encoder
from fastapi.responses import StreamingResponse

from atlas.context import AtlasContext
from atlas.core.crs import require_crs
from atlas.core.exceptions import InvalidFilter
from atlas.core.logging import OperationLog
from atlas.core.types import BBox
from atlas.layers.models import Layer
from atlas.layers.permissions import check_permission
from atlas.postgis.geometry import to_geojson_feature
from atlas.postgis.queries import bbox_predicate, build_feature_query, count_query
from atlas.wfs.filters import Filter, compile_filters
from atlas.wfs.pagination import resolve_pagination


def parse_property_names(value: str | None, layer: Layer) -> list[str] | None:
    if value is None or not value.strip():
        return None
    names = list(dict.fromkeys(name.strip() for name in value.split(",")))
    for name in names:
        if name not in layer.attribute_names:
            raise InvalidFilter("unknown attribute", column=name)
    return names


async def get_feature(
    ctx: AtlasContext,
    *,
    type_names: str,
    bbox: BBox | None = None,
    bbox_srid: int | None = None,
    output_srid: int = 4326,
    filters: list[Filter] | None = None,
    property_names: list[str] | None = None,
    include_geometry: bool = True,
    count: int | None = None,
    start_index: int = 0,
    principal: Any = None,
    report: OperationLog | None = None,
) -> StreamingResponse:
    layer = ctx.registry.require_service(type_names, "wfs")
    await check_permission(ctx.permission_checker, layer, "query", principal)
    require_crs(output_srid)
    page = resolve_pagination(ctx.config, count, start_index)
    where, params = compile_filters(filters or [], layer)
    if bbox is not None:
        predicate, values = bbox_predicate(
            layer.table_info, bbox, bbox_srid if bbox_srid is not None else ctx.config.default_srid
        )
        where.append(predicate)
        params.extend(values)
    query, values = count_query(layer.table_info, where, params)
    matched = await ctx.db.fetch_one(query, values)
    query, values = build_feature_query(
        layer.table_info,
        columns=property_names,
        where=where,
        params=params,
        target_srid=output_srid,
        geometry_as="geojson" if include_geometry else "none",
        limit=page.limit,
        offset=page.offset,
    )
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
            await rows.aclose()

    return StreamingResponse(body(), media_type="application/geo+json")
