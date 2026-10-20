"""Parse WMS 1.3.0 map parameters and delegate bounded, authorized drawing."""

import json
from typing import Any, Literal, Self

from fastapi import Response
from pydantic import BaseModel, ConfigDict, Field, ValidationInfo, field_validator, model_validator
from starlette.concurrency import run_in_threadpool

from atlas.context import AtlasContext
from atlas.core.config import AtlasConfig
from atlas.core.crs import require_crs
from atlas.core.exceptions import InvalidFilter, InvalidParameter, UnsupportedFormat
from atlas.core.logging import OperationLog
from atlas.core.predicates import Group, Predicate, PredicateModel, validate_predicate
from atlas.core.types import BBox
from atlas.layers.permissions import check_permission
from atlas.postgis.predicates import compile_predicate
from atlas.postgis.queries import fetch_render_geometries
from atlas.rendering.base import MapRenderer, RenderLayer
from atlas.rendering.pillow import PillowRenderer
from atlas.styles.models import Color, Style, default_style


def validate_layer_filters(value: Any, config: AtlasConfig | None = None) -> dict[str, Predicate]:
    limits = config or AtlasConfig(database_url="unused")

    def unique(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, item in pairs:
            if key in result:
                raise InvalidFilter("duplicate layer filter key")
            result[key] = item
        return result

    try:
        if isinstance(value, str):
            if len(value.encode("utf-8")) > limits.filter_max_bytes:
                raise InvalidFilter("layer filters exceed byte limit")
            value = json.loads(value, object_pairs_hook=unique)
        if not isinstance(value, dict) or any(not isinstance(key, str) for key in value):
            raise InvalidFilter("layer filters must be an object")
        encoded = json.dumps(
            value,
            default=lambda v: v.model_dump() if isinstance(v, PredicateModel) else v,
            ensure_ascii=False,
            allow_nan=False,
        )
        if len(encoded.encode("utf-8")) > limits.filter_max_bytes:
            raise InvalidFilter("layer filters exceed byte limit")
        if not value:
            return {}
        # The synthetic AND accounts for aggregate budgets without consuming a user node.
        group = validate_predicate(
            {"op": "and", "args": list(value.values())},
            max_depth=limits.filter_max_depth + 1,
            max_nodes=limits.filter_max_nodes + 1,
            max_bytes=limits.filter_max_bytes + 32,
            max_coordinates=limits.filter_max_coordinates,
            max_in_values=limits.filter_max_in_values,
        )
        assert isinstance(group, Group)
        return dict(zip(value, group.args, strict=True))
    except (ValueError, TypeError, RecursionError) as exc:
        raise InvalidFilter("invalid layer filters") from exc


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
    atlas_filters: dict[str, Predicate] = Field(default_factory=dict)

    @field_validator("atlas_filters", mode="before")
    @classmethod
    def layer_filters(cls, value: Any, info: ValidationInfo) -> dict[str, Predicate]:
        return validate_layer_filters(value, (info.context or {}).get("config"))

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
        if not set(self.atlas_filters).issubset(self.layers):
            raise InvalidFilter("filter layers must belong to LAYERS")
        if any(self.styles):
            raise ValueError("named WMS styles are not supported")
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
    styles: dict[str, Style] | None = None,
) -> Response:
    params.validate_size(ctx.config.max_image_size)
    validate_layer_filters(params.atlas_filters, ctx.config)
    layers = [ctx.registry.require_service(name, "wms") for name in params.layers]
    for layer in layers:
        await check_permission(ctx.permission_checker, layer, "view", principal)
    compiled = [
        compile_predicate(params.atlas_filters.get(layer.name), layer.table_info)
        for layer in layers
    ]
    render_layers = []
    for layer, (predicate, values) in zip(layers, compiled, strict=True):
        geometries = await fetch_render_geometries(
            ctx.db,
            layer.table_info,
            bbox=params.bbox,
            bbox_srid=params.crs,
            target_srid=params.crs,
            where=[predicate] if layer.name in params.atlas_filters else [],
            params=values,
        )
        render_layers.append(
            RenderLayer(
                geometries=geometries,
                style=(styles or {}).get(layer.name)
                or layer.style
                or default_style(layer.geometry_type),
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
