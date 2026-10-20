"""Convert attribute literals to PostgreSQL scalar types for query adapters."""

import math
from datetime import date, datetime
from decimal import Decimal
from typing import Any


def cast_attribute_value(value: Any, data_type: str) -> Any:
    text = str(value)
    if data_type in ("int2", "int4", "int8"):
        bits = {"int2": 16, "int4": 32, "int8": 64}[data_type]
        number = int(text)
        if not -(2 ** (bits - 1)) <= number < 2 ** (bits - 1):
            raise ValueError("integer out of range")
        return number
    if data_type in ("float4", "float8"):
        number_float = float(text)
        if not math.isfinite(number_float) or (
            data_type == "float4" and abs(number_float) > 3.4028235e38
        ):
            raise ValueError("non-finite number")
        return number_float
    if data_type in ("numeric", "decimal"):
        number_decimal = Decimal(text)
        if not number_decimal.is_finite():
            raise ValueError("non-finite number")
        return number_decimal
    if data_type == "bool":
        if text.lower() not in ("true", "false", "1", "0"):
            raise ValueError("invalid boolean")
        return text.lower() in ("true", "1")
    if data_type == "date":
        return date.fromisoformat(text)
    if data_type in ("timestamp", "timestamptz"):
        return datetime.fromisoformat(text)
    return text
