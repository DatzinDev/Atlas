"""Query only visible WMS layers within a pixel-scaled click window."""

from typing import Any, Literal, Self

from fastapi.encoders import jsonable_encoder
from pydantic import Field, field_validator, model_validator

from atlas.context import AtlasContext
from atlas.core.exceptions import UnsupportedFormat
from atlas.core.logging import OperationLog
from atlas.layers.permissions import check_permission
from atlas.postgis.geometry import to_geojson_feature
from atlas.postgis.predicates import compile_predicate
from atlas.postgis.queries import build_feature_query, intersects_bbox_predicate
from atlas.wms.get_map import GetMapParams, validate_layer_filters


class FeatureInfoParams(GetMapParams):
    query_layers: list[str] = Field(min_length=1)
    i: int = Field(ge=0)
    j: int = Field(ge=0)
    feature_count: int = Field(default=1, gt=0)
    info_format: Literal["application/json"] = "application/json"
    tolerance_pixels: float = Field(default=5, gt=0)
    include_geometry: bool = False

    @field_validator("query_layers", mode="before")
    @classmethod
    def split_query_layers(cls, value: Any) -> Any:
        return cls.split_names(value)

    @field_validator("info_format", mode="before")
    @classmethod
    def information_format(cls, value: Any) -> Any:
        if value != "application/json":
            raise UnsupportedFormat("GetFeatureInfo supports application/json only")
        return value

    @model_validator(mode="after")
    def validate_click(self) -> Self:
        if self.i >= self.width or self.j >= self.height:
            raise ValueError("pixel lies outside the image")
        if not set(self.query_layers).issubset(self.layers) or len(set(self.query_layers)) != len(
            self.query_layers
        ):
            raise ValueError("query_layers must be unique members of layers")
        return self


async def get_feature_info(
    ctx: AtlasContext,
    params: FeatureInfoParams,
    principal: Any = None,
    *,
    report: OperationLog | None = None,
) -> dict[str, Any]:
    params.validate_size(ctx.config.max_image_size)
    validate_layer_filters(params.atlas_filters, ctx.config)
    layers = [ctx.registry.require_service(name, "wms") for name in params.query_layers]
    for layer in layers:
        await check_permission(ctx.permission_checker, layer, "query", principal)
    compiled = [
        compile_predicate(params.atlas_filters.get(layer.name), layer.table_info)
        for layer in layers
    ]
    minx, miny, maxx, maxy = params.bbox
    pixel_x, pixel_y = (maxx - minx) / params.width, (maxy - miny) / params.height
    x, y = minx + (params.i + 0.5) * pixel_x, maxy - (params.j + 0.5) * pixel_y
    dx, dy = params.tolerance_pixels * pixel_x, params.tolerance_pixels * pixel_y
    bbox = (x - dx, y - dy, x + dx, y + dy)
    features = []
    for layer, (selection, selection_values) in zip(layers, compiled, strict=True):
        predicate, values = intersects_bbox_predicate(layer.table_info, bbox, params.crs)
        query, values = build_feature_query(
            layer.table_info,
            where=[predicate, selection] if layer.name in params.atlas_filters else [predicate],
            params=[*values, *selection_values],
            geometry_as="geojson" if params.include_geometry else "none",
            target_srid=4326,
            limit=ctx.config.resolve_limit(params.feature_count),
        )
        for row in await ctx.db.fetch_all(query, values):
            feature = to_geojson_feature(row, layer.id_column, layer.geometry_column)
            feature["layer"] = layer.name
            features.append(feature)
    if report is not None:
        report.feature_count = len(features)
    return {"type": "FeatureCollection", "features": jsonable_encoder(features)}
