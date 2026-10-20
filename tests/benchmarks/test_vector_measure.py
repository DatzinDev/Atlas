"""Load is bounded; timings exclude incomplete data and cancellations are drained."""

import asyncio

import pytest
from benchmarks.vector_measure import load, measure, resources, stability, summarize


def test_outcomes_are_never_merged_with_complete():
    result = summarize(
        [
            dict(outcome=k, ms=i, bytes=10, status=200)
            for i, k in enumerate(
                ["complete", "page", "incomplete", "rejected", "complete"], start=1
            )
        ]
    )
    assert result["outcomes"]["complete"] == dict(count=2, p50_ms=1, p95_ms=5, total_bytes=20)
    assert result["outcomes"]["incomplete"]["count"] == 1
    assert resources()["rss_bytes"] > 0


async def test_exact_sample_count_and_bounded_concurrency():
    active = peak = 0

    async def call():
        nonlocal active, peak
        active += 1
        peak = max(peak, active)
        await asyncio.sleep(0.001)
        active -= 1
        return dict(outcome="complete", status=200, ms=1, bytes=5)

    result = await measure(call, samples=11, warmup=2, concurrency=3)
    assert result["outcomes"]["complete"]["count"] == 11 and peak == 3
    assert active == 0 and result["first_request"]["status"] == 200
    with pytest.raises(ValueError):
        await load(call, 1, 33)


async def test_cancellation_rest_and_connection_recovery(bench_db):
    active = 0

    async def call():
        nonlocal active
        active += 1
        try:
            await asyncio.sleep(10)
        finally:
            active -= 1

    result = await stability(call, bench_db, 2, rounds=3)
    assert result["cancelled"] == 6 and active == 0
    assert all(r["active_database_sessions"] == 0 for r in result["rest"])
    assert all(r["extra_async_tasks"] == 0 for r in result["rest"])


async def test_failed_load_does_not_leave_workers_running():
    active = 0

    async def call():
        nonlocal active
        active += 1
        try:
            if active == 2:
                raise RuntimeError("failed request")
            await asyncio.sleep(10)
        finally:
            active -= 1

    with pytest.raises(RuntimeError, match="failed request"):
        await load(call, 10, 4)
    assert active == 0


def test_postgres_cgroup_observer(tmp_path):
    (tmp_path / "cpu.stat").write_text("usage_usec 1500000\n")
    (tmp_path / "memory.current").write_text("4096")
    (tmp_path / "memory.stat").write_text("anon 1024\nshmem 1024\nfile 2048\n")
    sample = resources(tmp_path)["postgres"]
    assert sample["cpu_seconds"] == 1.5 and sample["cgroup_memory_bytes"] == 4096
    assert sample["file_bytes"] == 2048


async def test_incomplete_preflight_has_no_successful_hot_samples():
    from unittest.mock import AsyncMock

    call = AsyncMock(return_value=dict(outcome="incomplete", status=200, ms=5, bytes=20))
    result = await measure(call, samples=100, warmup=10, concurrency=4)
    assert call.await_count == 1
    assert result["samples"] == 0 and result["requested_samples"] == 100
    assert result["outcomes"]["complete"]["count"] == 0
    call.return_value["outcome"] = "invalid"
    with pytest.raises(AssertionError, match="invalid preflight"):
        await measure(call, samples=100, warmup=10, concurrency=4)
