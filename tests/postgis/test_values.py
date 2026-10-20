"""Shared casts preserve WFS scalar conversions and range checks."""

from datetime import date, datetime
from decimal import Decimal, InvalidOperation

import pytest

from atlas.postgis.values import cast_attribute_value


@pytest.mark.parametrize(
    "kind,value,expected",
    [
        ("int2", "32767", 32767),
        ("int4", "-2147483648", -2147483648),
        ("int8", "9223372036854775807", 9223372036854775807),
        ("float4", "1.25", 1.25),
        ("float8", "2.5", 2.5),
        ("numeric", "0.1", Decimal("0.1")),
        ("decimal", "1", Decimal(1)),
        ("bool", "false", False),
        ("bool", "1", True),
        ("date", "2026-01-02", date(2026, 1, 2)),
        ("timestamp", "2026-01-02T03:04:05", datetime(2026, 1, 2, 3, 4, 5)),
        (
            "timestamptz",
            "2026-01-02T03:04:05+00:00",
            datetime.fromisoformat("2026-01-02T03:04:05+00:00"),
        ),
        ("text", "';DROP TABLE x;--", "';DROP TABLE x;--"),
    ],
)
def test_cast(kind, value, expected):
    assert cast_attribute_value(value, kind) == expected


@pytest.mark.parametrize(
    "kind,value",
    [
        ("int2", "32768"),
        ("int4", "2147483648"),
        ("int8", "9223372036854775808"),
        ("float4", "3.5e38"),
        ("float8", "nan"),
        ("numeric", "Infinity"),
        ("bool", "yes"),
        ("date", "2026-02-30"),
        ("timestamp", "yesterday"),
    ],
)
def test_invalid_cast(kind, value):
    with pytest.raises((ValueError, InvalidOperation, OverflowError)):
        cast_attribute_value(value, kind)
