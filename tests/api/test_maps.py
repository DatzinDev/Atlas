"""Map downloads reuse WMS, enforce access and leave configured styles unchanged."""

from io import BytesIO
from unittest.mock import Mock

import pytest
from PIL import Image

BODY = dict(
    layers=["municipios"],
    bbox=[-103.1, 20.4, -101.4, 21.6],
    width=100,
    height=80,
    crs=4326,
    format="png",
    transparent=True,
)


@pytest.mark.parametrize("format", ["png", "jpeg"])
async def test_image_download(client, format):
    response = await client.post("/atlas/maps/export", json=BODY | {"format": format})
    assert response.status_code == 200 and response.headers["content-type"] == f"image/{format}"
    assert Image.open(BytesIO(response.content)).size == (100, 80)
    assert response.headers["content-disposition"].startswith('attachment; filename="map-')
    assert response.headers["content-disposition"].endswith(f'.{format}"')


async def test_override_is_local_and_rest_axes_stay_xy(client, atlas):
    layer = atlas.context.registry.get("municipios")
    original = layer.model_dump()
    renderer = Mock()
    renderer.render.return_value = b"image"
    atlas.context.renderer = renderer
    response = await client.post(
        "/atlas/maps/export", json=BODY | {"styles": {"municipios": {"fill": "#ff0000"}}}
    )
    assert response.status_code == 200
    assert renderer.render.call_args.args[0][0].style.fill == "#ff0000"
    assert renderer.render.call_args.kwargs["bbox"] == (-103.1, 20.4, -101.4, 21.6)
    assert layer.model_dump() == original


async def test_limits_and_all_layer_permissions(client, atlas):
    assert (
        await client.post("/atlas/maps/export", json=BODY | {"width": 20000})
    ).status_code == 400
    atlas.context.permission_checker = lambda layer, op, user: layer.name == "municipios"
    response = await client.post(
        "/atlas/maps/export", json=BODY | {"layers": ["municipios", "escuelas"]}
    )
    assert response.status_code == 403
    response = await client.post(
        "/atlas/maps/export", json=BODY | {"styles": {"missing": {"fill": "#fff"}}}
    )
    assert response.status_code == 400
