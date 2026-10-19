"""Dispatch the WMS 1.3.0 KVP subset with host-provided authorization and logging."""

from typing import Annotated, Any

from fastapi import APIRouter, Depends, Request, Response
from fastapi.responses import JSONResponse
from pydantic import ValidationError

from atlas.api.routing import AtlasRoute
from atlas.context import AtlasContext
from atlas.core.exceptions import InvalidParameter
from atlas.core.logging import log_operation
from atlas.layers.permissions import visible_layers
from atlas.wms.capabilities import capabilities_xml
from atlas.wms.feature_info import FeatureInfoParams, get_feature_info
from atlas.wms.get_map import GetMapParams, get_map

MAP_FIELDS = {
    "layers",
    "bbox",
    "width",
    "height",
    "crs",
    "format",
    "styles",
    "transparent",
    "bgcolor",
}
INFO_FIELDS = MAP_FIELDS | {
    "query_layers",
    "i",
    "j",
    "feature_count",
    "info_format",
    "tolerance_pixels",
}


def build_router(ctx: AtlasContext) -> APIRouter:
    router = APIRouter(route_class=AtlasRoute)
    dependency = ctx.principal_dependency or (lambda: None)

    @router.get("/wms")
    async def wms(request: Request, principal: Annotated[Any, Depends(dependency)]) -> Response:
        pairs = list(request.query_params.multi_items())
        params = {key.lower(): value for key, value in pairs}
        operation = params.get("request", "")
        operation = {
            "getmap": "GetMap",
            "getfeatureinfo": "GetFeatureInfo",
            "getcapabilities": "GetCapabilities",
        }.get(operation.lower(), operation)
        user = str(principal) if isinstance(principal, (str, int)) else None
        async with log_operation(ctx.log_hook, operation, params.get("layers"), user) as report:
            report.feature_count = 0
            if len(params) != len(pairs):
                raise InvalidParameter("duplicate WMS parameter")
            if params.get("service", "WMS").upper() != "WMS":
                raise InvalidParameter("SERVICE must be WMS")
            if params.get("version", "1.3.0") != "1.3.0":
                raise InvalidParameter("only WMS 1.3.0 is supported")
            if operation == "GetCapabilities":
                layers = [
                    layer
                    for layer in await visible_layers(
                        ctx.registry, ctx.permission_checker, "view", principal
                    )
                    if layer.supports("wms")
                ]
                report.feature_count = len(layers)
                return Response(
                    capabilities_xml(layers, str(request.url).split("?", 1)[0]),
                    media_type="application/xml",
                )
            if operation not in ("GetMap", "GetFeatureInfo"):
                raise InvalidParameter("unknown or missing REQUEST", parameter="request")
            try:
                if operation == "GetMap":
                    map_params = GetMapParams.model_validate(
                        {key: value for key, value in params.items() if key in MAP_FIELDS}
                    )
                    return await get_map(ctx, map_params, principal, report=report)
                values = {key: value for key, value in params.items() if key in INFO_FIELDS}
                if "geometry" in params:
                    values["include_geometry"] = params["geometry"]
                info_params = FeatureInfoParams.model_validate(values)
                return JSONResponse(
                    await get_feature_info(ctx, info_params, principal, report=report)
                )
            except ValidationError as exc:
                raise InvalidParameter("invalid WMS parameters") from exc

    return router
