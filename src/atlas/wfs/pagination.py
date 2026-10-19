"""Resolve WFS and convenience pagination names against configured limits."""

from pydantic import BaseModel, Field

from atlas.core.config import AtlasConfig
from atlas.core.exceptions import InvalidFilter


class Pagination(BaseModel):
    limit: int = Field(gt=0)
    offset: int = Field(ge=0)


def resolve_pagination(
    config: AtlasConfig,
    count: int | None = None,
    start_index: int | None = None,
    limit: int | None = None,
    offset: int | None = None,
) -> Pagination:
    requested = count if count is not None else limit
    start = start_index if start_index is not None else offset if offset is not None else 0
    if start < 0:
        raise InvalidFilter("startIndex cannot be negative")
    try:
        effective = config.resolve_limit(requested)
    except ValueError as exc:
        raise InvalidFilter("count must be positive") from exc
    return Pagination(limit=effective, offset=start)
