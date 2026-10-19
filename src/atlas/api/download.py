"""Filtered downloads use a server-side cursor and never the default page limit."""

from collections.abc import AsyncGenerator
from typing import Annotated, Any, Literal, cast

from anyio import CancelScope
from fastapi import APIRouter, Depends, Request
from fastapi.responses import StreamingResponse
from starlette.types import Receive, Scope, Send

from atlas.api.routing import AtlasRoute
from atlas.context import AtlasContext
from atlas.core.exceptions import InvalidParameter, LayerNotFound, PermissionDenied, QueryTooLarge
from atlas.exports.base import download_filename, get_exporter
from atlas.exports.csv_export import CSVExporter
from atlas.layers.permissions import check_permission
from atlas.postgis.queries import bbox_predicate, build_feature_query, count_query, parse_bbox
from atlas.wfs.filters import compile_filters, parse_filters
from atlas.wfs.get_feature import parse_property_names


class _DownloadResponse(StreamingResponse):
    def __init__(
        self,
        body: AsyncGenerator[bytes, None],
        rows: AsyncGenerator[dict[str, Any], None],
        **kwargs: Any,
    ) -> None:
        super().__init__(body, **kwargs)
        self.rows = rows

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        try:
            await super().__call__(scope, receive, send)
        finally:
            # Headers may fail before the body starts its cleanup.
            with CancelScope(shield=True):
                await self.rows.aclose()


def build_router(ctx: AtlasContext) -> APIRouter:
    router = APIRouter(route_class=AtlasRoute)
    dependency = ctx.principal_dependency or (lambda: None)

    @router.get("/layers/{name}/download")
    async def download(
        name: str, request: Request, principal: Annotated[Any, Depends(dependency)]
    ) -> StreamingResponse:
        layer = ctx.registry.get(name)
        if not layer.enabled:
            raise LayerNotFound("the requested layer is not enabled", name=name)
        if not layer.downloadable:
            raise PermissionDenied("the layer is not downloadable", name=name)
        await check_permission(ctx.permission_checker, layer, "download", principal)
        params = dict(request.query_params)
        if len(params) != len(request.query_params.multi_items()):
            raise InvalidParameter("duplicate download parameter")
        exporter = get_exporter(params.get("format", "geojson"))
        if exporter.format == "csv":
            try:
                exporter = CSVExporter(
                    cast(Literal["wkt", "centroid", "none"], params.get("geometry", "wkt"))
                )
            except ValueError as exc:
                raise InvalidParameter("invalid CSV geometry mode") from exc
        columns = parse_property_names(params.pop("properties", None), layer)
        where, values = compile_filters(parse_filters(params, layer), layer)
        if "bbox" in params:
            bbox, srid = parse_bbox(params["bbox"])
            predicate, coordinates = bbox_predicate(
                layer.table_info, bbox, srid or ctx.config.default_srid
            )
            where.append(predicate)
            values.extend(coordinates)
        cap = ctx.config.max_download_features
        if cap is not None:
            query, bound = count_query(layer.table_info, where, values)
            count = await ctx.db.fetch_one(query, bound)
            if count is not None and count["count"] > cap:
                raise QueryTooLarge("download exceeds max_download_features", maximum=cap)
        query, bound = build_feature_query(
            layer.table_info,
            columns=columns,
            where=where,
            params=values,
            target_srid=4326,
            limit=cap,
        )
        attributes = [
            a
            for a in layer.attributes
            if columns is None or a.name in columns or a.name == layer.id_column
        ]
        metadata = layer.model_copy(update={"attributes": attributes, "srid": 4326})
        rows = cast(
            AsyncGenerator[dict[str, Any], None], ctx.db.stream(query, bound, batch_size=16)
        )
        try:
            first = await anext(rows, None)
        except BaseException:
            with CancelScope(shield=True):
                await rows.aclose()
            raise

        async def remaining() -> AsyncGenerator[dict[str, Any], None]:
            if first is not None:
                yield first
            async for row in rows:
                yield row

        async def body() -> AsyncGenerator[bytes, None]:
            try:
                async for chunk in exporter.export(remaining(), metadata):
                    yield chunk
            finally:
                with CancelScope(shield=True):
                    await rows.aclose()

        filename = download_filename(layer, exporter)
        return _DownloadResponse(
            body(),
            rows,
            media_type=exporter.media_type,
            headers={"Content-Disposition": f'attachment; filename="{filename}"'},
        )

    return router
