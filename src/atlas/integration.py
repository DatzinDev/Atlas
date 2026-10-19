"""The public integration with a host FastAPI application."""

from collections.abc import AsyncIterator, Callable
from contextlib import AbstractAsyncContextManager, asynccontextmanager
from typing import Any

from fastapi import APIRouter, FastAPI

from atlas.api.download import build_router as build_download_router
from atlas.api.layers import create_router
from atlas.api.maps import build_router as build_maps_router
from atlas.context import AtlasContext
from atlas.core.config import AtlasConfig
from atlas.core.logging import LogHook
from atlas.layers.metadata import build_layer
from atlas.layers.models import Layer
from atlas.layers.permissions import PermissionChecker
from atlas.layers.registry import LayerRegistry
from atlas.postgis.connection import Database
from atlas.rendering.base import MapRenderer
from atlas.rendering.pillow import PillowRenderer
from atlas.wfs.router import build_router as build_wfs_router
from atlas.wms.router import build_router as build_wms_router


class Atlas:
    def __init__(
        self,
        database_url: str,
        *,
        prefix: str = "/atlas",
        config: AtlasConfig | None = None,
        renderer: MapRenderer | None = None,
        permission_checker: PermissionChecker | None = None,
        principal_dependency: Callable[..., Any] | None = None,
        log_hook: LogHook | None = None,
    ) -> None:
        if config is not None and config.database_url != database_url:
            raise ValueError("database_url must match the supplied config")
        config = config if config is not None else AtlasConfig(database_url=database_url)
        self.context = AtlasContext(
            config=config,
            db=Database(config),
            registry=LayerRegistry(),
            renderer=renderer if renderer is not None else PillowRenderer(),
            permission_checker=permission_checker,
            principal_dependency=principal_dependency,
            log_hook=log_hook,
        )
        self._prefix = prefix
        self._router: APIRouter | None = None

    async def register_layer(self, source: str, **overrides: Any) -> Layer:
        return self.register(await build_layer(self.context.db, source, **overrides))

    def register(self, layer: Layer) -> Layer:
        return self.context.registry.register(layer)

    @property
    def router(self) -> APIRouter:
        if self._router is None:
            self._router = APIRouter(prefix=self._prefix)
            self._router.include_router(create_router(self.context))
            self._router.include_router(build_maps_router(self.context))
            self._router.include_router(build_download_router(self.context))
            self._router.include_router(build_wfs_router(self.context))
            self._router.include_router(build_wms_router(self.context))
        return self._router

    async def startup(self) -> None:
        await self.context.db.open()

    async def shutdown(self) -> None:
        await self.context.db.close()

    @property
    def lifespan(self) -> Callable[[FastAPI], AbstractAsyncContextManager[None]]:
        return self._lifespan

    @asynccontextmanager
    async def _lifespan(self, app: FastAPI) -> AsyncIterator[None]:
        await self.startup()
        try:
            yield
        finally:
            await self.shutdown()
