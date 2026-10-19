"""CRS parsing, alias axes and registry extensibility."""

import pytest

from atlas.core.crs import SUPPORTED_CRS, parse_crs, require_crs
from atlas.core.exceptions import UnsupportedCRS


@pytest.mark.parametrize(
    "value", ["EPSG:4326", "epsg:4326", "urn:ogc:def:crs:EPSG::4326", "4326", "CRS:84"]
)
def test_parse(value):
    assert parse_crs(value) == 4326


def test_axis_alias_does_not_mutate_registry():
    assert require_crs("CRS:84").axis_order == "xy"
    assert SUPPORTED_CRS[4326].axis_order == "yx"
    assert require_crs(3857).axis_order == "xy"


@pytest.mark.parametrize("value", ["EPSG:123", "garbage", "", "EPSG:4326;DROP", "-1"])
def test_unsupported(value):
    with pytest.raises(UnsupportedCRS) as error:
        parse_crs(value)
    assert error.value.details["supported"] == [4326, 3857]


def test_new_registry_entry_needs_no_parser_changes(monkeypatch):
    monkeypatch.setitem(SUPPORTED_CRS, 999, SUPPORTED_CRS[3857].model_copy(update={"srid": 999}))
    assert parse_crs("EPSG:999") == 999
