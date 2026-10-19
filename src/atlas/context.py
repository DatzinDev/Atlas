"""Per-instance state passed to Atlas routers, never stored globally."""

from collections.abc import Callable
from typing import Any

from pydantic import BaseModel, ConfigDict

from atlas.core.config import AtlasConfig
from atlas.core.logging import LogHook
from atlas.layers.permissions import PermissionChecker
from atlas.layers.registry import LayerRegistry
from atlas.postgis.connection import Database


class AtlasContext(BaseModel):
    model_config = ConfigDict(arbitrary_types_allowed=True)

    config: AtlasConfig
    db: Database
    registry: LayerRegistry
    # shortcut: the renderer type arrives with issue #29.
    renderer: Any = None
    permission_checker: PermissionChecker | None = None
    log_hook: LogHook | None = None
    principal_dependency: Callable[..., Any] | None = None
