"""Behavior of `AtlasConfig`: query limits, validation and connection options."""

from __future__ import annotations

import pytest
from pydantic import ValidationError

from atlas.core.config import AtlasConfig

# Any non-blank string works: AtlasConfig validates the URL as text and never connects.
# `.invalid` is reserved by RFC 2606, so this can never resolve to a real host.
DATABASE_URL = "postgresql://user:password@db.invalid/atlas"


def make_config(**overrides: object) -> AtlasConfig:
    return AtlasConfig(database_url=DATABASE_URL, **overrides)


class TestDefaults:
    def test_only_the_database_url_is_required(self) -> None:
        config = make_config()

        assert config.database_url == DATABASE_URL
        assert config.default_limit == 1000
        assert config.max_limit == 10000
        assert config.max_download_features is None
        assert config.default_srid == 4326
        assert config.pool_min_size == 1
        assert config.pool_max_size == 10
        assert config.query_timeout == 30.0
        assert config.max_image_size == 4096

    def test_database_url_is_mandatory(self) -> None:
        with pytest.raises(ValidationError):
            AtlasConfig()  # type: ignore[call-arg]


class TestStrictness:
    def test_an_unknown_field_is_rejected_instead_of_silently_ignored(self) -> None:
        """A typo must not leave the real limit at its default without a word."""
        with pytest.raises(ValidationError):
            make_config(max_limmit=50)

    def test_the_config_is_immutable(self) -> None:
        config = make_config()

        with pytest.raises(ValidationError):
            config.max_limit = -5  # type: ignore[misc]

        assert config.max_limit == 10000

    @pytest.mark.parametrize("url", ["", "   "])
    def test_a_blank_database_url_is_rejected(self, url: str) -> None:
        with pytest.raises(ValidationError):
            AtlasConfig(database_url=url)

    def test_surrounding_whitespace_in_the_database_url_is_stripped(self) -> None:
        """URLs routinely arrive from a .env file with a trailing newline."""
        assert AtlasConfig(database_url=f"  {DATABASE_URL}\n").database_url == DATABASE_URL


class TestResolveLimit:
    def test_no_requested_limit_falls_back_to_the_default(self) -> None:
        assert make_config().resolve_limit(None) == 1000

    def test_a_requested_limit_below_the_ceiling_is_honored(self) -> None:
        assert make_config().resolve_limit(50) == 50

    def test_a_requested_limit_above_the_ceiling_is_capped_instead_of_rejected(self) -> None:
        assert make_config().resolve_limit(999_999) == 10000

    def test_the_ceiling_itself_is_honored(self) -> None:
        assert make_config().resolve_limit(10000) == 10000

    @pytest.mark.parametrize("requested", [0, -1, -999])
    def test_non_positive_limits_are_rejected(self, requested: int) -> None:
        with pytest.raises(ValueError):
            make_config().resolve_limit(requested)

    def test_the_configured_default_is_used_not_the_hardcoded_one(self) -> None:
        config = make_config(default_limit=25, max_limit=100)

        assert config.resolve_limit(None) == 25
        assert config.resolve_limit(999) == 100


class TestFieldValidation:
    @pytest.mark.parametrize(
        "field",
        [
            "default_limit",
            "max_limit",
            "default_srid",
            "pool_min_size",
            "pool_max_size",
            "query_timeout",
            "max_image_size",
        ],
    )
    @pytest.mark.parametrize("value", [0, -1])
    def test_numeric_fields_reject_non_positive_values(self, field: str, value: int) -> None:
        with pytest.raises(ValidationError):
            make_config(**{field: value})

    def test_max_download_features_accepts_none_meaning_unbounded(self) -> None:
        assert make_config(max_download_features=None).max_download_features is None

    def test_max_download_features_accepts_a_positive_value(self) -> None:
        assert make_config(max_download_features=500).max_download_features == 500

    @pytest.mark.parametrize("value", [0, -1])
    def test_max_download_features_rejects_non_positive_values(self, value: int) -> None:
        with pytest.raises(ValidationError):
            make_config(max_download_features=value)


class TestPoolConsistency:
    def test_a_minimum_above_the_maximum_is_a_validation_error(self) -> None:
        with pytest.raises(ValidationError):
            make_config(pool_min_size=10, pool_max_size=5)

    def test_a_minimum_equal_to_the_maximum_is_allowed(self) -> None:
        config = make_config(pool_min_size=5, pool_max_size=5)

        assert (config.pool_min_size, config.pool_max_size) == (5, 5)

    def test_a_minimum_below_the_maximum_is_allowed(self) -> None:
        config = make_config(pool_min_size=2, pool_max_size=20)

        assert (config.pool_min_size, config.pool_max_size) == (2, 20)


class TestLimitConsistency:
    def test_a_default_limit_above_the_ceiling_is_a_validation_error(self) -> None:
        with pytest.raises(ValidationError):
            make_config(default_limit=5000, max_limit=1000)

    def test_a_default_limit_equal_to_the_ceiling_is_allowed(self) -> None:
        assert make_config(default_limit=1000, max_limit=1000).default_limit == 1000
