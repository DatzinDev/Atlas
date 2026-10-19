"""Page sizes are always bounded; aliases share the same validation."""

import pytest

from atlas import AtlasConfig
from atlas.core.exceptions import InvalidFilter
from atlas.wfs.pagination import resolve_pagination


@pytest.mark.parametrize(
    "kwargs,expected",
    [
        ({}, (5, 0)),
        ({"count": 100}, (10, 0)),
        ({"limit": 3, "offset": 2}, (3, 2)),
        ({"count": 2, "start_index": 4}, (2, 4)),
        ({"count": 2, "limit": 3, "start_index": 1, "offset": 4}, (2, 1)),
    ],
)
def test_bounded_aliases(kwargs, expected):
    page = resolve_pagination(
        AtlasConfig(database_url="test", default_limit=5, max_limit=10), **kwargs
    )
    assert (page.limit, page.offset) == expected


@pytest.mark.parametrize(
    "kwargs", [{"count": 0}, {"count": -1}, {"limit": 0}, {"start_index": -1}, {"offset": -1}]
)
def test_invalid_pagination(kwargs):
    with pytest.raises(InvalidFilter):
        resolve_pagination(AtlasConfig(database_url="test"), **kwargs)
