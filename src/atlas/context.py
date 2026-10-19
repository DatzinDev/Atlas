"""Per-instance state passed to Atlas routers, never stored globally."""

from collections.abc import Awaitable, Callable
from typing import Any

from pydantic import BaseModel, ConfigDict

from atlas.core.config import AtlasConfig
from atlas.layers.models import Layer
from atlas.layers.registry import LayerRegistry
from atlas.postgis.connection import Database

PermissionChecker = Callable[[Layer, str, Any], bool | Awaitable[bool]]


class AtlasContext(BaseModel):
    model_config = ConfigDict(arbitrary_types_allowed=True)

    config: AtlasConfig
    db: Database
    registry: LayerRegistry
    # shortcut: renderer and logging types arrive with issues #29 and #18.
    renderer: Any = None
    permission_checker: PermissionChecker | None = None
    log_hook: Callable[[Any], None] | None = None
    principal_dependency: Callable[..., Any] | None = None
