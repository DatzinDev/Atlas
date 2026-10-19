"""Layer routes resolve the current registry for every request."""

from typing import Annotated, Any

from fastapi import APIRouter, Depends
from pydantic import BaseModel, Field

from atlas.api.routing import AtlasRoute
from atlas.context import AtlasContext
from atlas.core.exceptions import LayerNotFound
from atlas.core.types import BBox
from atlas.layers.permissions import check_permission, visible_layers
from atlas.styles.models import Style


class LayerSummary(BaseModel):
    name: str
    title: str
    description: str | None
    geometry_type: str
    srid: int
    extent: BBox | None
    services: dict[str, bool]


class LayerListResponse(BaseModel):
    layers: list[LayerSummary]


class AttributeInfo(BaseModel):
    name: str
    type: str


class LayerDetailResponse(LayerSummary):
    attributes: list[AttributeInfo]
    # shortcut: advertise formats only when issue #37 supplies the exporter registry.
    download_formats: list[str] = Field(default_factory=list)
    style: Style | None
    min_zoom: int | None
    max_zoom: int | None


def create_router(context: AtlasContext) -> APIRouter:
    router = APIRouter(route_class=AtlasRoute)
    principal_dependency = context.principal_dependency or (lambda: None)

    @router.get("/layers")
    async def list_layers(
        principal: Annotated[Any, Depends(principal_dependency)],
    ) -> LayerListResponse:
        layers = await visible_layers(
            context.registry, context.permission_checker, "view", principal
        )
        return LayerListResponse(
            layers=[LayerSummary.model_validate(layer.model_dump()) for layer in layers]
        )

    @router.get("/layers/{name}")
    async def layer_detail(
        name: str, principal: Annotated[Any, Depends(principal_dependency)]
    ) -> LayerDetailResponse:
        layer = context.registry.get(name)
        if not layer.enabled:
            raise LayerNotFound("the requested layer is not enabled", name=name)
        await check_permission(context.permission_checker, layer, "query", principal)
        values = layer.model_dump()
        values["attributes"] = [
            {"name": a.name, "type": a.data_type} for a in layer.table_info.columns
        ]
        return LayerDetailResponse.model_validate(values)

    return router
