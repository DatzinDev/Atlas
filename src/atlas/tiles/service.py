"""Authorize vector tiles independently of WMS and WFS publication."""

from collections.abc import Sequence
from typing import Any

from atlas.context import AtlasContext
from atlas.core.predicates import Predicate
from atlas.core.tiles import TileCoord, tile_bounds
from atlas.layers.permissions import check_permission
from atlas.postgis.mvt import fetch_mvt


async def get_vector_tile(
    ctx: AtlasContext,
    name: str,
    tile: TileCoord,
    *,
    properties: Sequence[str] | None = None,
    predicate: Predicate | None = None,
    principal: Any = None,
) -> bytes:
    tile_bounds(tile, max_zoom=ctx.config.max_tile_zoom)
    layer = ctx.registry.require_service(name, "mvt")
    await check_permission(ctx.permission_checker, layer, "view", principal)
    await check_permission(ctx.permission_checker, layer, "query", principal)
    return await fetch_mvt(
        ctx.db,
        layer.table_info,
        tile,
        layer_name=layer.name,
        properties=properties,
        predicate=predicate,
    )
