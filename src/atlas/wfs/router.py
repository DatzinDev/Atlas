"""Dispatch Atlas's WFS KVP subset for GET and form-encoded POST requests."""

from collections.abc import Awaitable, Callable
from typing import Annotated, Any
from urllib.parse import parse_qsl

from fastapi import APIRouter, Depends, Request, Response
from fastapi.responses import JSONResponse
from starlette.types import Receive, Scope, Send

from atlas.api.routing import AtlasRoute
from atlas.context import AtlasContext
from atlas.core.crs import parse_crs
from atlas.core.exceptions import (
    AtlasError,
    InvalidFilter,
    InvalidParameter,
    UnsupportedFormat,
    error_response,
)
from atlas.core.logging import OperationLog, log_operation
from atlas.core.predicates import parse_predicate
from atlas.layers.permissions import check_permission, visible_layers
from atlas.postgis.queries import parse_bbox
from atlas.wfs.capabilities import capabilities_xml
from atlas.wfs.describe import describe_feature_type
from atlas.wfs.filters import RESERVED_PARAMS, parse_filters
from atlas.wfs.get_feature import get_feature, parse_property_names, parse_sort_by
from atlas.wfs.pagination import resolve_pagination


class _OperationResponse(Response):
    def __init__(
        self,
        ctx: AtlasContext,
        operation: str,
        layer: str | None,
        principal: Any,
        dispatch: Callable[[OperationLog], Awaitable[Response]],
    ) -> None:
        super().__init__()
        self.ctx, self.operation, self.layer, self.dispatch = ctx, operation, layer, dispatch
        self.user = str(principal) if isinstance(principal, (str, int)) else None

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        async with log_operation(
            self.ctx.log_hook, self.operation, self.layer, self.user
        ) as report:
            try:
                response = await self.dispatch(report)
            except AtlasError as exc:
                report.error = exc.code
                response = JSONResponse(error_response(exc), status_code=exc.status_code)
            await response(scope, receive, send)


def _integer(value: str | None) -> int | None:
    if value is None:
        return None
    try:
        return int(value)
    except ValueError as exc:
        raise InvalidFilter("pagination values must be integers") from exc


def build_router(ctx: AtlasContext) -> APIRouter:
    router = APIRouter(route_class=AtlasRoute)
    dependency = ctx.principal_dependency or (lambda: None)
    reserved = RESERVED_PARAMS | {"sortby", "atlas_filter", "atlas_filters", "cql_filter", "filter"}

    @router.get("/wfs")
    @router.post("/wfs")
    async def wfs(request: Request, principal: Annotated[Any, Depends(dependency)]) -> Response:
        pairs = list(request.query_params.multi_items())
        body = await request.body() if request.method == "POST" else b""
        if (
            body
            and request.headers.get("content-type", "").split(";")[0]
            == "application/x-www-form-urlencoded"
        ):
            try:
                pairs.extend(parse_qsl(body.decode("utf-8"), keep_blank_values=True))
            except UnicodeDecodeError as exc:
                raise InvalidParameter("form must be UTF-8 encoded") from exc
        params: dict[str, str] = {}
        for key, value in pairs:
            key = key.lower() if key.lower() in reserved else key
            if key in params:
                raise InvalidParameter("duplicate request parameter", parameter=key)
            params[key] = value
        operation = params.get("request", "")
        name = params.get("typenames", params.get("typename"))

        async def dispatch(report: OperationLog) -> Response:
            if (
                body
                and request.headers.get("content-type", "").split(";")[0]
                != "application/x-www-form-urlencoded"
            ):
                raise UnsupportedFormat("POST supports form encoding, not XML")
            if params.get("service", "WFS").upper() != "WFS":
                raise InvalidParameter("SERVICE must be WFS")
            if params.get("version", "2.0.0") != "2.0.0":
                raise InvalidParameter("only WFS 2.0.0 is supported")
            if operation.lower() == "getcapabilities":
                layers = await visible_layers(
                    ctx.registry, ctx.permission_checker, "view", principal
                )
                report.feature_count = len([layer for layer in layers if layer.supports("wfs")])
                return Response(
                    capabilities_xml(layers, str(request.url).split("?", 1)[0]),
                    media_type="application/xml",
                )
            if operation.lower() not in ("getfeature", "describefeaturetype"):
                raise InvalidParameter("unknown or missing REQUEST", parameter="request")
            if not name or "," in name:
                raise InvalidParameter("exactly one typeNames is required")
            layer = ctx.registry.require_service(name, "wfs")
            if operation.lower() == "describefeaturetype":
                await check_permission(ctx.permission_checker, layer, "query", principal)
                return JSONResponse(
                    describe_feature_type(layer, params.get("outputformat", "application/json"))
                )
            output_format = params.get("outputformat", params.get("format", "application/geo+json"))
            if output_format not in ("application/json", "application/geo+json"):
                raise UnsupportedFormat("GetFeature supports GeoJSON only")
            if params.get("resulttype", "results") != "results":
                raise InvalidParameter("resultType=hits is not supported")
            if {"cql_filter", "filter", "atlas_filters"} & params.keys():
                raise InvalidFilter("unsupported filter encoding; use ATLAS_FILTER JSON")
            geometry = params.get("geometry", "true").lower()
            if geometry not in ("true", "false", "1", "0"):
                raise InvalidParameter("geometry must be true or false")
            bbox, bbox_srid = parse_bbox(params["bbox"]) if "bbox" in params else (None, None)
            page = resolve_pagination(
                ctx.config,
                _integer(params.get("count")),
                _integer(params.get("startindex")),
                _integer(params.get("limit")),
                _integer(params.get("offset")),
            )
            response = await get_feature(
                ctx,
                type_names=name,
                bbox=bbox,
                bbox_srid=bbox_srid,
                output_srid=parse_crs(params.get("srsname", str(ctx.config.default_srid))),
                filters=parse_filters(
                    {k: v for k, v in params.items() if k not in {"sortby", "atlas_filter"}}, layer
                ),
                predicate=parse_predicate(params.get("atlas_filter"), ctx.config),
                order_by=parse_sort_by(params.get("sortby"), layer),
                property_names=parse_property_names(
                    params.get("propertyname"), layer, allow_geometry=True
                ),
                include_geometry=geometry in ("true", "1"),
                count=page.limit,
                start_index=page.offset,
                principal=principal,
                report=report,
            )
            response.headers["content-type"] = output_format
            return response

        return _OperationResponse(ctx, operation, name, principal, dispatch)

    return router
