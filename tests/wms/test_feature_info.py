"""Click coordinates, zoom-scaled tolerance, exact intersections and query permissions."""

import pytest
from pydantic import ValidationError

from atlas.core.exceptions import PermissionDenied, UnsupportedFormat
from atlas.wms.feature_info import FeatureInfoParams, get_feature_info


def params(**changes):
    return FeatureInfoParams.model_validate(
        dict(
            layers=["municipios"],
            query_layers=["municipios"],
            bbox=(20.5, -103, 21.5, -102),
            crs=4326,
            width=100,
            height=100,
            i=25,
            j=25,
        )
        | changes
    )


async def test_click_attributes_and_optional_geometry(atlas):
    data = await get_feature_info(atlas.context, params())
    feature = data["features"][0]
    assert feature["id"] == 1 and feature["properties"]["nombre"] == "San Bernardo"
    assert feature["geometry"] is None and feature["layer"] == "municipios"
    data = await get_feature_info(atlas.context, params(include_geometry=True))
    assert data["features"][0]["geometry"]["type"] == "Polygon"


@pytest.mark.parametrize(
    "changes",
    [
        dict(i=-1),
        dict(i=100),
        dict(j=100),
        dict(query_layers=["escuelas"]),
        dict(query_layers=["municipios", "municipios"]),
    ],
)
def test_invalid_click(changes):
    with pytest.raises(ValidationError):
        params(**changes)


async def test_count_per_layer_and_query_permission(atlas):
    atlas.context.permission_checker = lambda layer, op, user: op == "query"
    p = params(
        layers=["municipios", "escuelas"],
        query_layers="municipios,escuelas",
        width=2,
        height=2,
        i=0,
        j=0,
        feature_count=2,
    )
    features = (await get_feature_info(atlas.context, p))["features"]
    assert sum(f["layer"] == "municipios" for f in features) == 2
    assert sum(f["layer"] == "escuelas" for f in features) == 2
    atlas.context.permission_checker = lambda layer, op, user: op == "view"
    with pytest.raises(PermissionDenied):
        await get_feature_info(atlas.context, p)
    with pytest.raises(UnsupportedFormat):
        params(info_format="text/html")


async def test_tolerance_changes_with_zoom_and_pixel_aspect(atlas, monkeypatch):
    from atlas.wms.feature_info import intersects_bbox_predicate

    windows = []

    def observed(info, bbox, srid):
        windows.append(bbox)
        return intersects_bbox_predicate(info, bbox, srid)

    monkeypatch.setattr("atlas.wms.feature_info.intersects_bbox_predicate", observed)
    await get_feature_info(atlas.context, params())
    await get_feature_info(atlas.context, params(bbox=(21.1, -102.9, 21.3, -102.5)))
    first, second = windows
    assert first == pytest.approx((-102.795, 21.195, -102.695, 21.295))
    assert second[2] - second[0] == pytest.approx(0.04)
    assert second[3] - second[1] == pytest.approx(0.02)
