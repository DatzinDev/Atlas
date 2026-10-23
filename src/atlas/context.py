"""Per-instance state passed to Atlas routers, never stored globally."""

from asyncio import Semaphore
from collections.abc import AsyncIterator, Callable
from contextlib import asynccontextmanager
from typing import Any

from pydantic import BaseModel, ConfigDict, PrivateAttr

from atlas.core.config import AtlasConfig
from atlas.core.exceptions import QueryTooLarge
from atlas.core.logging import LogHook
from atlas.layers.permissions import PermissionChecker
from atlas.layers.registry import LayerRegistry
from atlas.postgis.connection import Database


class AtlasContext(BaseModel):
    model_config = ConfigDict(arbitrary_types_allowed=True)

    config: AtlasConfig
    db: Database
    registry: LayerRegistry
    # External duck-typed renderers need no runtime inheritance check.
    renderer: Any = None
    permission_checker: PermissionChecker | None = None
    log_hook: LogHook | None = None
    principal_dependency: Callable[..., Any] | None = None

    _render_slots: Semaphore = PrivateAttr()

    def model_post_init(self, __context: Any) -> None:
        self._render_slots = Semaphore(self.config.max_render_concurrency)

    @asynccontextmanager
    async def render_slot(self) -> AsyncIterator[None]:
        # Acquisition cannot suspend when unlocked: there is no admission queue.
        if self._render_slots.locked():
            raise QueryTooLarge("render capacity exhausted; retry later")
        async with self._render_slots:
            yield
