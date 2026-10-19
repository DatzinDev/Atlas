"""Independent operations and fail-closed sync/async authorization."""

import pytest

from atlas import Layer
from atlas.core.exceptions import PermissionDenied
from atlas.layers.permissions import check_permission, default_permission_name, visible_layers
from atlas.layers.registry import LayerRegistry


@pytest.fixture
def item() -> Layer:
    return Layer(
        name="test",
        schema_name="geo",
        table_name="test",
        geometry_column="geom",
        geometry_type="Point",
        srid=4326,
    )


@pytest.mark.parametrize("op", ["view", "query", "download", "admin"])
async def test_default_allow_and_independent_operations(item, op) -> None:
    await check_permission(None, item, op, None)
    assert default_permission_name(item, op) == f"atlas.test.{op}"

    def checker(layer, operation, principal):
        return operation == op

    for operation in ["view", "query", "download", "admin"]:
        if operation == op:
            await check_permission(checker, item, operation, "user")
        else:
            with pytest.raises(PermissionDenied):
                await check_permission(checker, item, operation, "user")


async def test_async_checker_and_principal(item: Layer) -> None:
    async def checker(layer, op, principal):
        assert layer is item and op == "query"
        return principal == "allowed"

    await check_permission(checker, item, "query", "allowed")
    with pytest.raises(PermissionDenied):
        await check_permission(checker, item, "query", "denied")


@pytest.mark.parametrize("asynchronous", [False, True])
async def test_checker_errors_deny(item: Layer, asynchronous: bool) -> None:
    def broken(*args):
        raise RuntimeError("secret credentials")

    async def async_broken(*args):
        raise RuntimeError("secret credentials")

    with pytest.raises(PermissionDenied) as raised:
        await check_permission(async_broken if asynchronous else broken, item, "view", None)
    assert "secret" not in str(raised.value)


async def test_truthy_non_boolean_does_not_grant_access(item: Layer) -> None:
    with pytest.raises(PermissionDenied):
        await check_permission(lambda *args: "yes", item, "view", None)


async def test_visible_filters_denied_and_disabled(item: Layer) -> None:
    registry = LayerRegistry()
    registry.register(item)
    assert await visible_layers(registry, None, "view", None) == [item]
    assert await visible_layers(registry, lambda *args: False, "view", None) == []
    item.enabled = False
    assert await visible_layers(registry, None, "view", None) == []
