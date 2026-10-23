"""Real response coverage plus deliberate corruption and partial-response detection."""

from types import SimpleNamespace

import httpx
import pytest
from benchmarks.vector_checks import mosaic, observe, reference, request, validate
from benchmarks.vector_data import GLOBAL, MOSAIC, SELECTIVE, dataset
from fastapi import FastAPI

from atlas import Atlas


@pytest.mark.parametrize("kind", ["point", "line", "polygon"])
async def test_real_protocols_and_mosaic(bench_db, kind):
    atlas = Atlas(bench_db._config.database_url, log_hook=observe)
    atlas.context.db = bench_db
    app = FastAPI()
    app.include_router(atlas.router)
    async with dataset(bench_db, 1600, kind) as layer:
        atlas.register(layer)
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app), base_url="http://test"
        ) as client:
            for tile in (GLOBAL, SELECTIVE):
                expected = await reference(atlas.context, layer, tile)
                for protocol in ("mvt", "wfs", "wms"):
                    result = await request(
                        client, protocol, layer, tile, expected, atlas.context.config
                    )
                    assert result["outcome"] == (
                        "page" if protocol == "wfs" and tile == GLOBAL else "complete"
                    )
            coverage = await mosaic(atlas.context, client, layer, MOSAIC, 1600)
            assert coverage["unique_ids"] == 1600


def test_corrupted_ids_and_oversized_success_fail():
    config = SimpleNamespace(default_limit=1000)
    response = httpx.Response(200, json={"features": [], "numberMatched": 2, "numberReturned": 1})
    with pytest.raises(AssertionError):
        validate(response, "wfs", "point", {"matched": 2, "page_ids": [1]}, config)
    with pytest.raises(AssertionError):
        validate(httpx.Response(200, content=b""), "mvt", "point", {"encoded_ids": None}, config)
    assert validate(httpx.Response(413), "mvt", "point", {}, config) == "rejected"
    assert validate(httpx.Response(504), "mvt", "point", {}, config) == "error"


async def test_repeated_payload_uses_digest_but_changes_are_revalidated(monkeypatch):
    from unittest.mock import AsyncMock, Mock

    spy = Mock(wraps=validate)
    monkeypatch.setattr("benchmarks.vector_checks.validate", spy)
    client = AsyncMock()
    client.get.return_value = httpx.Response(200, content=b"")
    expected = {"encoded_ids": set()}
    config = SimpleNamespace(default_limit=1000)
    layer = SimpleNamespace(name="point")
    for _ in range(2):
        assert (await request(client, "mvt", layer, GLOBAL, expected, config))[
            "outcome"
        ] == "complete"
    assert spy.call_count == 1
    client.get.return_value = httpx.Response(200, content=b"broken")
    assert (await request(client, "mvt", layer, GLOBAL, expected, config))["outcome"] == "invalid"
    assert spy.call_count == 2


def test_wms_partial_is_never_complete():
    from io import BytesIO

    from PIL import Image

    output = BytesIO()
    Image.new("RGBA", (256, 256)).save(output, format="PNG")
    response = httpx.Response(200, content=output.getvalue())
    for actual, outcome in [(1000, "incomplete"), (1001, "complete")]:
        assert (
            validate(
                response,
                "wms",
                "point",
                {"matched": 1001},
                SimpleNamespace(default_limit=1000),
                [actual],
            )
            == outcome
        )


@pytest.mark.parametrize("count", [1000, 1001, 1100, 1101])
async def test_complete_map_or_explicit_feature_rejection(bench_db, count):
    from unittest.mock import Mock

    from benchmarks.vector_data import dataset

    from atlas.core.config import AtlasConfig

    atlas = Atlas(bench_db._config.database_url)
    atlas.context.db = bench_db
    app = FastAPI()
    app.include_router(atlas.router)
    config = AtlasConfig(
        database_url=bench_db._config.database_url,
        default_limit=1,
        max_limit=1,
        max_render_features=1100,
    )
    atlas.context.config = atlas.context.db._config = config
    renderer = Mock()
    renderer.render.return_value = b"image"
    atlas.context.renderer = renderer
    async with (
        dataset(bench_db, count, "point") as layer,
        httpx.AsyncClient(transport=httpx.ASGITransport(app), base_url="http://test") as client,
    ):
        atlas.register(layer)
        response = await client.get(
            "/atlas/wms",
            params=dict(
                REQUEST="GetMap",
                LAYERS=layer.name,
                CRS="EPSG:3857",
                BBOX="-20037508,-20037508,20037508,20037508",
                WIDTH=16,
                HEIGHT=16,
            ),
        )
        hits = await client.get(
            "/atlas/wfs",
            params=dict(
                REQUEST="GetFeature",
                TYPENAMES=layer.name,
                RESULTTYPE="hits",
            ),
        )
        from xml.etree import ElementTree as ET

        assert hits.status_code == 200
        assert int(ET.fromstring(hits.content).attrib["numberMatched"]) == count
        if count <= 1100:
            assert response.status_code == 200
            assert len(renderer.render.call_args.args[0][0].geometries) == count
        else:
            assert response.status_code == 413
            assert response.json()["error"]["code"] == "query_too_large"
            renderer.render.assert_not_called()
