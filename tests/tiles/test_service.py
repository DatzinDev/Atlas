"""Service flags and both permissions precede every MVT database call."""

from unittest.mock import AsyncMock

import pytest

from atlas.core.exceptions import LayerNotFound, PermissionDenied, ServiceDisabled
from atlas.core.predicates import Comparison
from atlas.core.tiles import TileCoord
from atlas.tiles.service import get_vector_tile


async def test_service_authorizes_before_database(atlas, monkeypatch):
    fetch = AsyncMock(return_value=b"tile")
    monkeypatch.setattr("atlas.tiles.service.fetch_mvt", fetch)
    tile = TileCoord(z=0, x=0, y=0)
    layer = atlas.context.registry.get("municipios")
    assert layer.services.wms and layer.services.wfs and not layer.services.mvt
    with pytest.raises(LayerNotFound):
        await get_vector_tile(atlas.context, "missing", tile)
    with pytest.raises(ServiceDisabled):
        await get_vector_tile(atlas.context, layer.name, tile)
    layer.services.mvt = True
    layer.enabled = False
    with pytest.raises(ServiceDisabled):
        await get_vector_tile(atlas.context, layer.name, tile)
    layer.enabled = True
    for denied in ("view", "query"):
        atlas.context.permission_checker = lambda layer, op, user, denied=denied: op != denied
        with pytest.raises(PermissionDenied):
            await get_vector_tile(atlas.context, layer.name, tile)
    fetch.assert_not_awaited()
    seen = []
    atlas.context.permission_checker = lambda layer, op, user: seen.append((op, user)) or True
    predicate = Comparison(op="eq", field="id", value=1)
    assert (
        await get_vector_tile(
            atlas.context,
            layer.name,
            tile,
            properties=["nombre"],
            predicate=predicate,
            principal="reader",
        )
        == b"tile"
    )
    assert seen == [("view", "reader"), ("query", "reader")]
    assert fetch.call_args.kwargs == dict(
        layer_name="municipios", properties=["nombre"], predicate=predicate
    )
