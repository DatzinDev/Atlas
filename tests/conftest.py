"""Shared fixtures: database pool, layers built from the seed dataset, and an HTTP client.

Every fixture is skipped when its prerequisite is missing, so the suite reports skips instead of
errors on a machine without the development container up, or before the component it needs has
landed.
"""

from __future__ import annotations

import importlib
import os
from collections.abc import AsyncIterator
from typing import TYPE_CHECKING, Any

import pytest

if TYPE_CHECKING:
    from httpx import AsyncClient

    from atlas import Atlas, Layer
    from atlas.postgis.connection import Database

DATABASE_URL_ENV = "ATLAS_TEST_DATABASE_URL"


def _require(module: str, attribute: str) -> Any:
    """Return `module.attribute`, skipping the test while the component is not implemented."""
    try:
        return getattr(importlib.import_module(module), attribute)
    except (ImportError, AttributeError):
        pytest.skip(f"{module}.{attribute} is not implemented yet")


@pytest.fixture(scope="session")
def database_url() -> str:
    url = os.environ.get(DATABASE_URL_ENV)
    if not url:
        pytest.skip(f"{DATABASE_URL_ENV} is not set; run `docker compose up -d` first")
    return url


@pytest.fixture
async def db(database_url: str) -> AsyncIterator[Database]:
    atlas_config = _require("atlas.core.config", "AtlasConfig")
    database_class = _require("atlas.postgis.connection", "Database")

    database = database_class(atlas_config(database_url=database_url))
    await database.open()
    try:
        yield database
    finally:
        await database.close()


async def _build_layer(db: Database, source: str) -> Layer:
    build_layer = _require("atlas.layers.metadata", "build_layer")
    return await build_layer(db, source)


@pytest.fixture
async def municipios_layer(db: Database) -> Layer:
    return await _build_layer(db, "geo.municipios")


@pytest.fixture
async def carreteras_layer(db: Database) -> Layer:
    return await _build_layer(db, "geo.carreteras")


@pytest.fixture
async def escuelas_layer(db: Database) -> Layer:
    return await _build_layer(db, "geo.escuelas")


@pytest.fixture
async def atlas(
    database_url: str,
    municipios_layer: Layer,
    carreteras_layer: Layer,
    escuelas_layer: Layer,
) -> AsyncIterator[Atlas]:
    atlas_class = _require("atlas", "Atlas")

    instance = atlas_class(database_url=database_url)
    await instance.startup()
    try:
        for layer in (municipios_layer, carreteras_layer, escuelas_layer):
            instance.register(layer)
        yield instance
    finally:
        await instance.shutdown()


@pytest.fixture
async def client(atlas: Atlas) -> AsyncIterator[AsyncClient]:
    fastapi = _require("fastapi", "FastAPI")
    httpx = importlib.import_module("httpx")

    app = fastapi()
    app.include_router(atlas.router)
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://testserver") as http_client:
        yield http_client
