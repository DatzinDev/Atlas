"""Catalog replacement, lookup errors, services and deterministic listing."""

import pytest

from atlas.core.exceptions import LayerNotFound, ServiceDisabled
from atlas.layers.models import Layer
from atlas.layers.registry import LayerRegistry


def layer(name: str = "municipios", **overrides: object) -> Layer:
    return Layer.model_validate(
        {
            "name": name,
            "schema_name": "geo",
            "table_name": name,
            "geometry_column": "geom",
            "geometry_type": "Polygon",
            "srid": 4326,
            **overrides,
        }
    )


def test_register_replaces_and_returns_the_new_layer() -> None:
    registry = LayerRegistry()
    assert len(registry) == 0 and "municipios" not in registry
    first, replacement = layer(), layer(title="Replacement")
    assert registry.register(first) is first
    assert registry.register(replacement) is replacement
    assert registry.get("municipios") is replacement
    assert len(registry) == 1 and "municipios" in registry
    assert len(LayerRegistry()) == 0


@pytest.mark.parametrize("operation", ["get", "remove", "require_service"])
def test_missing_layer_has_name_details(operation: str) -> None:
    registry = LayerRegistry()
    with pytest.raises(LayerNotFound) as raised:
        if operation == "require_service":
            registry.require_service("missing", "wms")
        else:
            getattr(registry, operation)("missing")
    assert raised.value.details == {"name": "missing"}


@pytest.mark.parametrize("settings", [{"services": {"wms": False}}, {"enabled": False}])
def test_disabled_service_or_layer(settings: dict[str, object]) -> None:
    registry = LayerRegistry()
    registry.register(layer(**settings))
    with pytest.raises(ServiceDisabled):
        registry.require_service("municipios", "wms")


def test_enabled_and_unknown_services() -> None:
    registry = LayerRegistry()
    item = registry.register(layer())
    assert registry.require_service("municipios", "wms") is item
    with pytest.raises(ServiceDisabled):
        registry.require_service("municipios", "unknown")


def test_lists_are_sorted_filtered_and_independent() -> None:
    registry = LayerRegistry()
    for name, enabled in [("z", True), ("b", False), ("a", True)]:
        registry.register(layer(name, enabled=enabled))
    assert [item.name for item in registry.list()] == ["a", "z"]
    assert [item.name for item in registry.list(enabled_only=False)] == ["a", "b", "z"]
    registry.list().clear()
    assert len(registry) == 3
    registry.register(layer("a", title="Replacement"))
    assert [item.name for item in registry.list()] == ["a", "z"]


def test_remove() -> None:
    registry = LayerRegistry()
    registry.register(layer())
    registry.remove("municipios")
    assert len(registry) == 0 and "municipios" not in registry
    assert registry.list() == []
