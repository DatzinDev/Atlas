"""CLI budgets, explicit database selection and a small end-to-end matrix."""

import json

import pytest
from benchmarks.mvt import arguments, run


def test_cli_rejects_implicit_database_and_invalid_budgets(monkeypatch, tmp_path):
    monkeypatch.delenv("ATLAS_BENCH_DATABASE_URL", raising=False)
    with pytest.raises(SystemExit):
        arguments(["--output", str(tmp_path / "out.json")])
    monkeypatch.setenv("ATLAS_BENCH_DATABASE_URL", "postgresql://example/atlas_bench_test")
    for value in (["--samples", "0"], ["--rows", "1000001"], ["--concurrency", "33"]):
        with pytest.raises(SystemExit):
            arguments(["--output", str(tmp_path / "out.json"), *value])


async def test_end_to_end_cli_smoke(bench_db, monkeypatch, tmp_path):
    monkeypatch.setenv("ATLAS_BENCH_DATABASE_URL", bench_db._config.database_url)
    monkeypatch.delenv("ATLAS_BENCH_POSTGRES_CGROUP", raising=False)

    # Cancel/drain logic has a dedicated real-pool test; keep the CLI regression short.
    async def stable(*args, **kwargs):
        return {"rest": [dict(active_database_sessions=0, extra_async_tasks=0, pool={})]}

    monkeypatch.setattr("benchmarks.mvt.stability", stable)
    path = tmp_path / "out.json"
    args = arguments(
        [
            "--output",
            str(path),
            "--samples",
            "1",
            "--warmup",
            "0",
            "--rows",
            "16",
            "--repetitions",
            "1",
            "--concurrency",
            "1",
        ]
    )
    assert await run(args) == 0
    result = json.loads(path.read_text())
    assert result["finished"] and len(result["cases"]) == 18
    assert len(result["coverage"]) == 3
    assert "database_url" not in result["config"]
    assert "postgresql://" not in path.read_text()
