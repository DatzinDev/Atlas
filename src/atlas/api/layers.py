"""Layer routes resolve the current registry for every request."""

from fastapi import APIRouter

from atlas.api.routing import AtlasRoute
from atlas.context import AtlasContext


def create_router(context: AtlasContext) -> APIRouter:
    router = APIRouter(route_class=AtlasRoute)

    @router.get("/layers")
    async def list_layers() -> dict[str, list[dict[str, str]]]:
        return {
            "layers": [
                {"name": layer.name, "title": layer.title} for layer in context.registry.list()
            ]
        }

    return router
