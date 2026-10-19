"""Parse WMS 1.3.0 map parameters and delegate bounded, authorized drawing."""

from typing import Any, Literal, Self

from fastapi import Response
from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator
from starlette.concurrency import run_in_threadpool

from atlas.context import AtlasContext
from atlas.core.crs import require_crs
from atlas.core.exceptions import InvalidParameter, UnsupportedFormat
from atlas.core.logging import OperationLog
from atlas.core.types import BBox
from atlas.layers.permissions import check_permission
from atlas.postgis.queries import fetch_render_geometries
from atlas.rendering.base import MapRenderer, RenderLayer
from atlas.rendering.pillow import PillowRenderer
from atlas.styles.models import Color, default_style


class GetMapParams(BaseModel):
    model_config = ConfigDict(extra="forbid", allow_inf_nan=False)

    layers: list[str] = Field(min_length=1)
    bbox: BBox
    width: int = Field(gt=0)
    height: int = Field(gt=0)
    crs: int
    format: Literal["image/png", "image/jpeg"] = "image/png"
    styles: list[str] = Field(default_factory=list)
    transparent: bool = False
    bgcolor: Color = "#FFFFFF"

    @model_validator(mode="before")
    @classmethod
    def normalize_axes(cls, value: Any) -> Any:
        if not isinstance(value, dict):
            return value
        data = dict(value)
        if "crs" in data:
            crs = require_crs(data["crs"])
            data["crs"] = crs.srid
            if "bbox" in data:
                bbox = data["bbox"]
                if isinstance(bbox, str):
                    bbox = tuple(float(part) for part in bbox.split(","))
                if len(bbox) == 4 and crs.axis_order == "yx":
                    bbox = (bbox[1], bbox[0], bbox[3], bbox[2])
                data["bbox"] = bbox
        return data

    @field_validator("layers", "styles", mode="before")
    @classmethod
    def split_names(cls, value: Any) -> Any:
        return value.split(",") if isinstance(value, str) else value

    @field_validator("format", mode="before")
    @classmethod
    def image_format(cls, value: Any) -> Any:
        if value not in ("image/png", "image/jpeg"):
            raise UnsupportedFormat("GetMap supports PNG and JPEG only")
        return value

    @field_validator("bgcolor", mode="before")
    @classmethod
    def background_color(cls, value: Any) -> Any:
        if isinstance(value, str) and value.lower().startswith("0x"):
            return "#" + value[2:]
        return value

    @model_validator(mode="after")
    def validate_map(self) -> Self:
        if not all(self.layers) or len(set(self.layers)) != len(self.layers):
            raise ValueError("layers must be nonempty and unique")
        if self.bbox[0] >= self.bbox[2] or self.bbox[1] >= self.bbox[3]:
            raise ValueError("bbox must contain ordered bounds")
        return self

    def validate_size(self, maximum: int) -> None:
        if self.width > maximum or self.height > maximum:
            raise InvalidParameter("image dimensions exceed max_image_size")


async def get_map(
    ctx: AtlasContext,
    params: GetMapParams,
    principal: Any = None,
    *,
    report: OperationLog | None = None,
) -> Response:
    params.validate_size(ctx.config.max_image_size)
    layers = [ctx.registry.require_service(name, "wms") for name in params.layers]
    for layer in layers:
        await check_permission(ctx.permission_checker, layer, "view", principal)
    render_layers = []
    for layer in layers:
        geometries = await fetch_render_geometries(
            ctx.db,
            layer.table_info,
            bbox=params.bbox,
            bbox_srid=params.crs,
            target_srid=params.crs,
        )
        render_layers.append(
            RenderLayer(
                geometries=geometries, style=layer.style or default_style(layer.geometry_type)
            )
        )
    if report is not None:
        report.feature_count = sum(len(layer.geometries) for layer in render_layers)
    renderer: MapRenderer = ctx.renderer if ctx.renderer is not None else PillowRenderer()
    data = await run_in_threadpool(
        renderer.render,
        render_layers,
        bbox=params.bbox,
        width=params.width,
        height=params.height,
        image_format="png" if params.format == "image/png" else "jpeg",
        transparent=params.transparent,
        background=params.bgcolor,
    )
    return Response(data, media_type=params.format)
