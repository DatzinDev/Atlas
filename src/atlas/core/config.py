"""Single source of truth for every Atlas limit and connection option."""

from __future__ import annotations

from typing import Annotated

from pydantic import BaseModel, ConfigDict, Field, StringConstraints, model_validator

DatabaseURL = Annotated[str, StringConstraints(strip_whitespace=True, min_length=1)]


class AtlasConfig(BaseModel):
    """Query limits, connection options and rendering bounds for an Atlas instance.

    Every limit Atlas enforces lives here. Components read the values from this object instead of
    defining constants of their own.

    The model is frozen and rejects unknown fields on purpose: a misspelled option must fail loudly
    instead of leaving the real limit at its default, and a limit must not be reachable by mutating
    a shared config past its validators.
    """

    model_config = ConfigDict(extra="forbid", frozen=True)

    database_url: DatabaseURL
    default_limit: int = Field(default=1000, gt=0)
    max_limit: int = Field(default=10000, gt=0)
    max_download_features: int | None = Field(default=None, gt=0)
    default_srid: int = Field(default=4326, gt=0)
    pool_min_size: int = Field(default=1, gt=0)
    pool_max_size: int = Field(default=10, gt=0)
    query_timeout: float = Field(default=30.0, gt=0)
    max_image_size: int = Field(default=4096, gt=0)

    @model_validator(mode="after")
    def _check_limits_are_consistent(self) -> AtlasConfig:
        if self.default_limit > self.max_limit:
            raise ValueError(
                f"default_limit ({self.default_limit}) cannot exceed max_limit ({self.max_limit})"
            )
        return self

    @model_validator(mode="after")
    def _check_pool_sizes_are_consistent(self) -> AtlasConfig:
        if self.pool_min_size > self.pool_max_size:
            raise ValueError(
                f"pool_min_size ({self.pool_min_size}) cannot exceed "
                f"pool_max_size ({self.pool_max_size})"
            )
        return self

    def resolve_limit(self, requested: int | None) -> int:
        """Return the number of features to serve for a request.

        A missing limit falls back to `default_limit`; anything above `max_limit` is capped rather
        than rejected, so an oversized page still returns data. A non-positive limit is a client
        error and raises `ValueError`.
        """
        if requested is None:
            return self.default_limit
        if requested <= 0:
            raise ValueError(f"limit must be positive, got {requested}")
        return min(requested, self.max_limit)
