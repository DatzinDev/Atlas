"""Logging never masks request failures or creates failures of its own."""

import pytest

from atlas.core.exceptions import InvalidFilter
from atlas.core.logging import log_operation


async def test_success_and_mutable_count():
    reports = []
    async with log_operation(reports.append, "GetFeature", "test", "user") as report:
        report.feature_count = 3
    assert reports == [report]
    assert report.execution_time >= 0 and report.feature_count == 3
    assert report.error is None and report.user == "user"


@pytest.mark.parametrize(
    "error,code", [(InvalidFilter("bad"), "invalid_filter"), (ValueError("bad"), "ValueError")]
)
async def test_errors_are_logged_and_reraised(error, code):
    reports = []
    with pytest.raises(type(error)):
        async with log_operation(reports.append, "GetFeature"):
            raise error
    assert reports[0].error == code


async def test_broken_hook_cannot_break_request():
    def broken(report):
        raise RuntimeError("logging unavailable")

    async with log_operation(broken, "GetFeature"):
        pass
    with pytest.raises(ValueError, match="original"):
        async with log_operation(broken, "GetFeature"):
            raise ValueError("original")


async def test_no_hook_avoids_timing():
    async with log_operation(None, "GetFeature") as report:
        report.feature_count = 4
    assert report.execution_time == 0
