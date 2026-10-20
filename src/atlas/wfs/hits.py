"""Count authorized matches without opening a feature cursor or building geometry output."""

from datetime import UTC, datetime
from typing import Any
from xml.etree.ElementTree import Element, tostring

from atlas.context import AtlasContext
from atlas.core.predicates import Predicate
from atlas.core.types import BBox
from atlas.layers.permissions import check_permission
from atlas.postgis.queries import count_query
from atlas.wfs.filters import Filter, compile_query_filters


async def get_hits(
    ctx: AtlasContext,
    *,
    type_names: str,
    predicate: Predicate | None = None,
    filters: list[Filter] | None = None,
    bbox: BBox | None = None,
    bbox_srid: int | None = None,
    principal: Any = None,
) -> int:
    layer = ctx.registry.require_service(type_names, "wfs")
    await check_permission(ctx.permission_checker, layer, "query", principal)
    where, params = compile_query_filters(
        layer,
        filters=filters,
        predicate=predicate,
        bbox=bbox,
        bbox_srid=bbox_srid if bbox_srid is not None else ctx.config.default_srid,
    )
    query, values = count_query(layer.table_info, where, params)
    row = await ctx.db.fetch_one(query, values)
    return int(row["count"]) if row is not None else 0


def hits_xml(matched: int, next_url: str | None = None) -> str:
    root = Element(
        "wfs:FeatureCollection",
        {
            "xmlns:wfs": "http://www.opengis.net/wfs/2.0",
            "numberMatched": str(matched),
            "numberReturned": "0",
            "timeStamp": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
        },
    )
    if next_url is not None:
        root.set("next", next_url)
    return tostring(root, encoding="unicode", xml_declaration=True)
