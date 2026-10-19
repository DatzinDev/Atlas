"""Host app integration, late registration, local errors and pool lifecycle."""

import pytest
from fastapi import FastAPI
from fastapi.responses import JSONResponse
from httpx import ASGITransport, AsyncClient

from atlas import Atlas, AtlasConfig, Layer
from atlas.api.routing import AtlasRoute
from atlas.core.exceptions import AtlasError, LayerNotFound


async def test_late_registration_and_real_database(database_url: str) -> None:
    geo = Atlas(database_url)
    app = FastAPI(lifespan=geo.lifespan)
    app.include_router(geo.router)
    assert geo.router is geo.router
    async with app.router.lifespan_context(app):
        assert geo.context.db.pool is not None
        await geo.register_layer("geo.municipios")
        async with AsyncClient(transport=ASGITransport(app), base_url="http://test") as client:
            response = await client.get("/atlas/layers")
            assert response.status_code == 200
            assert response.json()["layers"][0]["name"] == "municipios"
    assert geo.context.db.pool is None


async def test_shutdown_on_lifespan_failure(database_url: str) -> None:
    geo = Atlas(database_url)
    app = FastAPI(lifespan=geo.lifespan)
    with pytest.raises(RuntimeError):
        async with app.router.lifespan_context(app):
            raise RuntimeError("host failed")
    assert geo.context.db.pool is None


async def test_prefix_errors_and_host_isolation() -> None:
    geo = Atlas("postgresql://unused", prefix="/geo")

    async def failure():
        raise LayerNotFound("missing", name="missing")

    geo.router.add_api_route("/failure", failure, route_class_override=AtlasRoute)
    app = FastAPI()
    app.include_router(geo.router)
    app.add_api_route("/host", failure)

    @app.exception_handler(AtlasError)
    async def host_error(request, exc):
        return JSONResponse({"host": True}, status_code=418)

    async with AsyncClient(transport=ASGITransport(app), base_url="http://test") as client:
        assert (await client.get("/geo/layers")).status_code == 200
        response = await client.get("/geo/failure")
        assert response.status_code == 404
        assert response.json() == {
            "error": {
                "code": "layer_not_found",
                "message": "missing",
                "details": {"name": "missing"},
            }
        }
        assert (await client.get("/host")).status_code == 418
        assert (await client.get("/atlas/layers")).status_code == 404


def test_instances_and_config_are_independent() -> None:
    geo, other = Atlas("postgresql://unused"), Atlas("postgresql://unused")
    item = Layer(
        name="test",
        schema_name="geo",
        table_name="test",
        geometry_column="geom",
        geometry_type="Point",
        srid=4326,
    )
    assert geo.register(item) is item
    assert len(other.context.registry) == 0
    with pytest.raises(ValueError):
        Atlas("postgresql://one", config=AtlasConfig(database_url="postgresql://other"))
