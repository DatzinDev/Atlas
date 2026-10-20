"""XYZ MVT transport with host principal injection and bounded filters."""

from typing import Annotated, Any

from fastapi import APIRouter, Depends, Request, Response
from pydantic import ValidationError

from atlas.api.routing import AtlasRoute
from atlas.context import AtlasContext
from atlas.core.exceptions import InvalidParameter
from atlas.core.predicates import parse_predicate
from atlas.core.tiles import TileCoord, tile_bounds
from atlas.tiles.service import get_vector_tile


def build_router(ctx: AtlasContext) -> APIRouter:
    router = APIRouter(route_class=AtlasRoute)
    dependency = ctx.principal_dependency or (lambda: None)

    @router.get("/tiles/{name}/{z}/{x}/{y}.mvt")
    async def tile(
        request: Request,
        name: str,
        z: str,
        x: str,
        y: str,
        principal: Annotated[Any, Depends(dependency)],
    ) -> Response:
        try:
            if any(not v.isascii() or not v.isdecimal() for v in (z, x, y)):
                raise ValueError("XYZ coordinates must be integers")
            coord = TileCoord(z=int(z), x=int(x), y=int(y))
        except (ValueError, ValidationError) as exc:
            raise InvalidParameter("invalid XYZ coordinates") from exc
        tile_bounds(coord, max_zoom=ctx.config.max_tile_zoom)
        pairs = list(request.query_params.multi_items())
        params = {key.lower(): value for key, value in pairs}
        if len(params) != len(pairs) or set(params) - {"properties", "atlas_filter"}:
            raise InvalidParameter("unsupported or duplicate tile parameter")
        properties = None
        if "properties" in params:
            properties = [name.strip() for name in params["properties"].split(",")]
            if not all(properties) or len(set(properties)) != len(properties):
                raise InvalidParameter("properties must be nonempty and unique")
        predicate = parse_predicate(params.get("atlas_filter"), ctx.config)
        data = await get_vector_tile(
            ctx, name, coord, properties=properties, predicate=predicate, principal=principal
        )
        return Response(
            data,
            media_type="application/vnd.mapbox-vector-tile",
            headers={"Cache-Control": "no-store"},
        )

    return router
