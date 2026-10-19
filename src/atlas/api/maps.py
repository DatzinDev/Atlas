"""Export a map through the existing WMS pipeline with request-local style overrides."""

from datetime import UTC, datetime
from typing import Annotated, Any, Literal

from fastapi import APIRouter, Depends, Response
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from atlas.api.routing import AtlasRoute
from atlas.context import AtlasContext
from atlas.core.exceptions import InvalidParameter
from atlas.core.types import BBox
from atlas.styles.models import Style
from atlas.wms.get_map import GetMapParams, get_map


class MapExportRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", allow_inf_nan=False)

    layers: list[str] = Field(min_length=1)
    bbox: BBox
    width: int = Field(gt=0)
    height: int = Field(gt=0)
    crs: int = 3857
    format: Literal["png", "jpeg"] = "png"
    transparent: bool = False
    styles: dict[str, Style] | None = None


def build_router(ctx: AtlasContext) -> APIRouter:
    router = APIRouter(route_class=AtlasRoute)
    dependency = ctx.principal_dependency or (lambda: None)

    @router.post("/maps/export")
    async def export_map(
        body: MapExportRequest,
        principal: Annotated[Any, Depends(dependency)],
    ) -> Response:
        if body.styles and not set(body.styles).issubset(body.layers):
            raise InvalidParameter("style overrides must target requested layers")
        # The REST bbox is always xy; CRS:84 avoids WMS's EPSG:4326 axis swap.
        try:
            params = GetMapParams.model_validate(
                {
                    "layers": body.layers,
                    "bbox": body.bbox,
                    "width": body.width,
                    "height": body.height,
                    "crs": "CRS:84" if body.crs == 4326 else body.crs,
                    "format": "image/png" if body.format == "png" else "image/jpeg",
                    "transparent": body.transparent,
                }
            )
        except ValidationError as exc:
            raise InvalidParameter("invalid map export parameters") from exc
        response = await get_map(ctx, params, principal, styles=body.styles)
        timestamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%S%fZ")
        response.headers["Content-Disposition"] = (
            f'attachment; filename="map-{timestamp}.{body.format}"'
        )
        return response

    return router
