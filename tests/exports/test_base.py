"""Registry lookup, optional dependencies and safe attachment filenames."""

from unittest.mock import Mock

import pytest

from atlas.core.exceptions import UnsupportedFormat
from atlas.exports import base
from atlas.layers.models import Layer


@pytest.fixture
def registry(monkeypatch):
    exporter = Mock(format="csv", media_type="text/csv", extension="csv", streaming=True)
    monkeypatch.setattr(base, "EXPORTERS", {"csv": exporter, "gpkg": exporter, "shp": exporter})
    return exporter


def test_registry_and_optional_dependency(registry, monkeypatch):
    monkeypatch.setattr(base, "find_spec", lambda name: None)
    assert base.available_formats() == ["csv"]
    assert base.get_exporter("csv") is registry
    with pytest.raises(UnsupportedFormat) as error:
        base.get_exporter("xml")
    assert error.value.status_code == 400 and error.value.details["formats"] == ["csv"]
    with pytest.raises(UnsupportedFormat):
        base.get_exporter("gpkg")
    monkeypatch.setattr(base, "find_spec", lambda name: object())
    assert base.available_formats() == ["csv", "gpkg", "shp"]


def test_filename_is_safe(registry):
    layer = Layer(
        name="safe",
        schema_name="geo",
        table_name="safe",
        geometry_column="geom",
        geometry_type="Point",
        srid=4326,
    )
    hostile = layer.model_copy(update={"name": "../../datos de salud/.."})
    name = base.download_filename(hostile, registry)
    assert name == "datos_de_salud.csv" and "/" not in name and ".." not in name and " " not in name
