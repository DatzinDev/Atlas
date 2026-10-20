"""Bounded load and Linux resource observations; no extra monitoring dependency."""

import asyncio
import math
import os
import resource
from collections import Counter
from pathlib import Path
from time import perf_counter

from psycopg import sql


def resources(cgroup=None):
    usage = resource.getrusage(resource.RUSAGE_SELF)
    proc = Path("/proc/self")
    result = {
        "cpu_seconds": usage.ru_utime + usage.ru_stime,
        "rss_bytes": int((proc / "statm").read_text().split()[1]) * os.sysconf("SC_PAGE_SIZE"),
        "process_peak_rss_bytes": usage.ru_maxrss * 1024,
        "threads": len(list((proc / "task").iterdir())),
        "fds": len(list((proc / "fd").iterdir())),
    }
    if cgroup:
        path = Path(cgroup)
        cpu = dict(line.split() for line in (path / "cpu.stat").read_text().splitlines())
        memory = dict(line.split() for line in (path / "memory.stat").read_text().splitlines())
        result["postgres"] = {
            "cpu_seconds": int(cpu["usage_usec"]) / 1e6,
            "cgroup_memory_bytes": int((path / "memory.current").read_text()),
            "anon_bytes": int(memory["anon"]),
            "shmem_bytes": int(memory["shmem"]),
            "file_bytes": int(memory["file"]),
        }
    return result


def summarize(samples):
    outcomes = {}
    for outcome in ("complete", "page", "incomplete", "rejected", "error", "invalid"):
        group = [s for s in samples if s["outcome"] == outcome]
        times = sorted(s["ms"] for s in group)
        outcomes[outcome] = {"count": len(group)}
        if times:
            outcomes[outcome].update(
                p50_ms=times[math.ceil(len(times) * 0.5) - 1],
                p95_ms=times[math.ceil(len(times) * 0.95) - 1],
                total_bytes=sum(s["bytes"] for s in group),
            )
    return {"outcomes": outcomes, "statuses": dict(Counter(s["status"] for s in samples))}


async def load(call, samples, concurrency):
    if samples < 1 or not 1 <= concurrency <= 32:
        raise ValueError("invalid load budget")
    remaining = iter(range(samples))
    result = []

    async def worker():
        for _ in remaining:
            result.append(await call())

    tasks = [asyncio.create_task(worker()) for _ in range(concurrency)]
    try:
        await asyncio.gather(*tasks)
    finally:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
    return result


async def measure(call, *, samples, warmup, concurrency, cgroup=None):
    first = await call()
    for _ in range(warmup):
        await call()
    observed = []

    async def sample():
        while True:
            observed.append(resources(cgroup))
            await asyncio.sleep(0.05)

    before = resources(cgroup)
    monitor = asyncio.create_task(sample())
    start = perf_counter()
    try:
        records = await load(call, samples, concurrency)
    finally:
        monitor.cancel()
        await asyncio.gather(monitor, return_exceptions=True)
    elapsed = perf_counter() - start
    after = resources(cgroup)
    result = {
        "first_request": first,
        "wall_seconds": elapsed,
        "samples": samples,
        "concurrency": concurrency,
        "before": before,
        "after": after,
        "cpu_seconds": after["cpu_seconds"] - before["cpu_seconds"],
        "sampled_peak_rss_bytes": max(s["rss_bytes"] for s in [before, after, *observed]),
        **summarize(records),
    }
    if cgroup:
        result["postgres_cpu_seconds"] = (
            after["postgres"]["cpu_seconds"] - before["postgres"]["cpu_seconds"]
        )
        result["postgres_sampled_peak_cgroup_bytes"] = max(
            s["postgres"]["cgroup_memory_bytes"] for s in [before, after, *observed]
        )
    return result


async def stability(call, db, concurrency, *, rounds=10, cgroup=None):
    """Cancel owned tasks repeatedly; observe native work after coroutine cancellation."""
    series = []
    cancelled = completed = 0
    baseline_tasks = len(asyncio.all_tasks())
    for _ in range(rounds):
        tasks = [asyncio.create_task(call()) for _ in range(concurrency)]
        await asyncio.sleep(0.01)
        for task in tasks:
            task.cancel()
        results = await asyncio.gather(*tasks, return_exceptions=True)
        cancelled += sum(isinstance(r, asyncio.CancelledError) for r in results)
        completed += sum(isinstance(r, dict) for r in results)
        series.append(resources(cgroup))
    rest = []
    for delay in (0, 0.2, 1):
        await asyncio.sleep(delay)
        pending = await db.fetch_one(
            sql.SQL("""
            SELECT count(*) AS n FROM pg_stat_activity WHERE datname=current_database()
                AND pid<>pg_backend_pid() AND state<>'idle'
        """).format()
        )
        rest.append(
            {
                **resources(cgroup),
                "active_database_sessions": pending["n"],
                "pool": db.pool.get_stats(),
                "extra_async_tasks": len(asyncio.all_tasks()) - baseline_tasks,
            }
        )
    return {
        "rounds": rounds,
        "cancelled": cancelled,
        "completed_before_cancel": completed,
        "series": series,
        "rest": rest,
    }
