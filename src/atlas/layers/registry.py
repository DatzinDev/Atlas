"""The in-memory catalog populated by the host application at startup."""

from atlas.core.exceptions import LayerNotFound, ServiceDisabled
from atlas.layers.models import Layer


class LayerRegistry:
    def __init__(self) -> None:
        self._layers: dict[str, Layer] = {}

    def register(self, layer: Layer) -> Layer:
        self._layers[layer.name] = layer
        return layer

    def get(self, name: str) -> Layer:
        try:
            return self._layers[name]
        except KeyError:
            raise LayerNotFound("the requested layer is not registered", name=name) from None

    def require_service(self, name: str, service: str) -> Layer:
        layer = self.get(name)
        if not layer.supports(service):
            raise ServiceDisabled("the requested service is disabled", name=name, service=service)
        return layer

    def list(self, *, enabled_only: bool = True) -> list[Layer]:
        return [
            self._layers[name]
            for name in sorted(self._layers)
            if not enabled_only or self._layers[name].enabled
        ]

    def remove(self, name: str) -> None:
        self.get(name)
        del self._layers[name]

    def __contains__(self, name: str) -> bool:
        return name in self._layers

    def __len__(self) -> int:
        return len(self._layers)
