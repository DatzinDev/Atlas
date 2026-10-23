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
