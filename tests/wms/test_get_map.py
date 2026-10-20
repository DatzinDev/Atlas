"""WMS axes, image formats, layer ordering, limits and access checks."""

from io import BytesIO
from unittest.mock import Mock

import pytest
from PIL import Image
from pydantic import ValidationError

from atlas.core.exceptions import (
    InvalidParameter,
    PermissionDenied,
    ServiceDisabled,
    UnsupportedFormat,
)
from atlas.wms.get_map import GetMapParams, get_map


def params(**overrides):
    return GetMapParams.model_validate(
        dict(
            layers=["municipios"],
            bbox=(20.4, -103.1, 21.6, -101.4),
            crs=4326,
            width=120,
            height=80,
            **overrides,
        )
    )


@pytest.mark.parametrize(
    "crs,bbox",
    [
        (4326, (20, -103, 22, -101)),
        ("EPSG:4326", "20,-103,22,-101"),
        ("CRS:84", "-103,20,-101,22"),
        (3857, (-103, 20, -101, 22)),
    ],
)
def test_axis_order(crs, bbox):
    data = GetMapParams(layers=["municipios"], bbox=bbox, crs=crs, width=10, height=10)
    assert data.bbox == (-103, 20, -101, 22)


@pytest.mark.parametrize(
    "changes",
    [
        dict(width=0),
        dict(height=-1),
        dict(bbox="nan,0,1,1"),
        dict(bbox="0,0,0,1"),
        dict(layers=["municipios", "municipios"]),
    ],
)
def test_invalid_parameters(changes):
    data = dict(layers=["municipios"], bbox=(0, 0, 1, 1), crs=3857, width=10, height=10)
    with pytest.raises(ValidationError):
        GetMapParams.model_validate(data | changes)


async def test_formats_and_empty_extent(atlas):
    for image_format in ("image/png", "image/jpeg"):
        p = GetMapParams(
            layers=["municipios"],
            bbox=(0, 0, 1, 1),
            crs=3857,
            width=40,
            height=30,
            format=image_format,
            transparent=True,
            bgcolor="0x00FF00",
        )
        response = await get_map(atlas.context, p)
        image = Image.open(BytesIO(response.body))
        assert response.media_type == image_format and image.size == (40, 30)
        if image_format == "image/png":
            assert image.getpixel((20, 15))[3] == 0
        else:
            assert image.getpixel((20, 15)) == pytest.approx((0, 255, 0), abs=2)
    with pytest.raises(UnsupportedFormat):
        params(format="image/gif")


async def test_layer_order_reprojection_and_renderer_injection(atlas):
    renderer = Mock()
    renderer.render.return_value = b"custom image"
    atlas.context.renderer = renderer
    response = await get_map(
        atlas.context,
        GetMapParams(
            layers="escuelas,municipios",
            bbox="20.4,-103.1,21.6,-101.4",
            crs="EPSG:4326",
            width=100,
            height=100,
            styles=",",
            transparent=True,
        ),
    )
    assert response.body == b"custom image"
    layers = renderer.render.call_args.args[0]
    assert layers[0].geometries[0].geom_type == "Point"
    assert layers[0].geometries[0].x == pytest.approx(-102.8, abs=0.01)
    assert layers[1].geometries[0].geom_type == "Polygon"
    assert renderer.render.call_args.kwargs["bbox"] == (-103.1, 20.4, -101.4, 21.6)


async def test_limits_and_permissions_before_query(atlas, monkeypatch):
    fetch = Mock(side_effect=AssertionError("must not query"))
    monkeypatch.setattr("atlas.wms.get_map.fetch_render_geometries", fetch)
    large = GetMapParams(
        layers=["municipios"],
        bbox=(0, 0, 1, 1),
        crs=3857,
        width=atlas.context.config.max_image_size + 1,
        height=1,
    )
    with pytest.raises(InvalidParameter):
        await get_map(atlas.context, large)
    layer = atlas.context.registry.get("municipios")
    layer.services.wms = False
    with pytest.raises(ServiceDisabled):
        await get_map(atlas.context, params())
    layer.services.wms = True
    atlas.context.permission_checker = lambda *args: False
    with pytest.raises(PermissionDenied):
        await get_map(atlas.context, params())
    fetch.assert_not_called()


def test_layer_filter_validation_and_aggregate_limits():
    from atlas.core.config import AtlasConfig
    from atlas.core.exceptions import InvalidFilter
    from atlas.wms.get_map import validate_layer_filters

    leaf = {"op": "eq", "field": "id", "value": 1}
    assert params(atlas_filters={"municipios": leaf}).atlas_filters["municipios"].value == 1
    for value in (
        "",
        "null",
        "[]",
        '{"a":{},"a":{}}',
        '{"a":{"op":"eq","field":"id","value":NaN}}',
    ):
        with pytest.raises(InvalidFilter):
            validate_layer_filters(value)
    with pytest.raises(InvalidFilter):
        params(atlas_filters={"other": leaf})
    config = AtlasConfig(database_url="unused", filter_max_nodes=1)
    with pytest.raises(InvalidFilter):
        validate_layer_filters({"a": leaf, "b": leaf}, config)
    assert len(validate_layer_filters({"a": leaf}, config)) == 1
    with pytest.raises(InvalidFilter):
        validate_layer_filters(
            {"a": {"op": "not", "arg": leaf}}, config.model_copy(update={"filter_max_depth": 1})
        )


async def test_layer_filters_intermediate_rejection(client, atlas, monkeypatch):
    import json

    from tests.wms.test_router import MAP

    from atlas.wms.feature_info import FeatureInfoParams, get_feature_info

    leaf = {"op": "eq", "field": "id", "value": 1}
    fetch = Mock(side_effect=AssertionError("must not query"))
    monkeypatch.setattr(atlas.context.db, "fetch_all", fetch)
    for operation in ("GetFeatureInfo",):
        response = await client.get(
            "/atlas/wms",
            params=MAP
            | dict(
                REQUEST=operation,
                QUERY_LAYERS="municipios",
                I=0,
                J=0,
                ATLAS_FILTERS=json.dumps({"municipios": leaf}),
            ),
        )
        assert response.status_code == 400
        assert "not implemented" in response.text
    with pytest.raises(InvalidParameter):
        await get_feature_info(
            atlas.context,
            FeatureInfoParams(
                layers=["municipios"],
                query_layers=["municipios"],
                bbox=(0, 0, 1, 1),
                crs=3857,
                width=10,
                height=10,
                i=0,
                j=0,
                atlas_filters={"municipios": leaf},
            ),
        )
    fetch.assert_not_called()


async def test_shared_filters_two_layers_match_wfs_and_do_not_leak(client, atlas):
    import json

    from shapely.geometry import shape
    from tests.wms.test_router import MAP

    renderer = Mock()
    renderer.render.return_value = b"image"
    atlas.context.renderer = renderer
    a = {"op": "in", "field": "id", "value": [1, 3]}
    b = {"op": "eq", "field": "id", "value": 2}
    for first in (a, b, a):
        filters = {"municipios": first, "escuelas": b}
        response = await client.get(
            "/atlas/wms",
            params=MAP | dict(LAYERS="municipios,escuelas", ATLAS_FILTERS=json.dumps(filters)),
        )
        assert response.status_code == 200
        rendered = renderer.render.call_args.args[0]
        for index, name in enumerate(filters):
            wfs = await client.get(
                "/atlas/wfs",
                params=dict(
                    REQUEST="GetFeature", TYPENAMES=name, ATLAS_FILTER=json.dumps(filters[name])
                ),
            )
            expected = [shape(f["geometry"]) for f in wfs.json()["features"]]
            assert len(rendered[index].geometries) == len(expected)
            assert all(
                actual.equals_exact(wanted, 1e-8)
                for actual, wanted in zip(rendered[index].geometries, expected, strict=True)
            )


async def test_filtered_map_rejects_all_invalid_columns_before_any_query(
    client, atlas, monkeypatch
):
    import json

    from tests.wms.test_router import MAP

    fetch = Mock(side_effect=AssertionError("must not query"))
    monkeypatch.setattr(atlas.context.db, "fetch_all", fetch)
    response = await client.get(
        "/atlas/wms",
        params=MAP
        | dict(
            LAYERS="municipios,escuelas",
            ATLAS_FILTERS=json.dumps(
                {"escuelas": {"op": "eq", "field": "id; DROP TABLE geo.escuelas", "value": 1}}
            ),
        ),
    )
    assert response.status_code == 400
    fetch.assert_not_called()
