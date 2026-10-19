"""Delegate authorization to the host application, denying on checker failures."""

import inspect
from collections.abc import Awaitable, Callable
from typing import Any, Literal

from atlas.core.exceptions import PermissionDenied
from atlas.layers.models import Layer
from atlas.layers.registry import LayerRegistry

Operation = Literal["view", "query", "download", "admin"]
PermissionChecker = Callable[[Layer, Operation, Any], bool | Awaitable[bool]]


def default_permission_name(layer: Layer, op: Operation) -> str:
    return f"atlas.{layer.name}.{op}"


async def check_permission(
    checker: PermissionChecker | None, layer: Layer, op: Operation, principal: Any
) -> None:
    """Without a checker all operations are allowed; with one, only True grants access."""
    if checker is None:
        return
    try:
        allowed = checker(layer, op, principal)
        if inspect.isawaitable(allowed):
            allowed = await allowed
    except Exception as exc:
        raise PermissionDenied("permission denied", name=layer.name, operation=op) from exc
    if allowed is not True:
        raise PermissionDenied("permission denied", name=layer.name, operation=op)


async def visible_layers(
    registry: LayerRegistry, checker: PermissionChecker | None, op: Operation, principal: Any
) -> list[Layer]:
    visible = []
    for layer in registry.list():
        try:
            await check_permission(checker, layer, op, principal)
        except PermissionDenied:
            continue
        visible.append(layer)
    return visible
