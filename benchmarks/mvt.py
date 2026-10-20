"""Run #140's vector benchmark against an explicitly disposable PostGIS database."""

import argparse
import asyncio
import hashlib
import importlib.metadata
import json
import os
import platform
import subprocess
import sys
from functools import partial
from pathlib import Path

if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import httpx  # noqa: E402
from fastapi import FastAPI  # noqa: E402
from psycopg import sql  # noqa: E402

from atlas import Atlas  # noqa: E402
from benchmarks.vector_checks import mosaic, observe, reference, request  # noqa: E402
from benchmarks.vector_data import GLOBAL, MOSAIC, SELECTIVE, dataset  # noqa: E402
from benchmarks.vector_measure import measure, stability  # noqa: E402


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=100)
    parser.add_argument("--warmup", type=int, default=10)
    parser.add_argument("--concurrency", type=int, nargs="+", default=[1, 4])
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--rows", type=int, nargs="+", default=[10000, 100000, 1000000])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    if (
        args.samples < 1
        or args.warmup < 0
        or args.repetitions < 1
        or any(not 1 <= c <= 32 for c in args.concurrency)
        or any(not 1 <= n <= 1000000 for n in args.rows)
    ):
        parser.error("positive budgets required; concurrency <=32 and rows <=1000000")
    if not __debug__:
        parser.error("benchmark integrity checks require Python without -O")
    if not os.environ.get("ATLAS_BENCH_DATABASE_URL"):
        parser.error("ATLAS_BENCH_DATABASE_URL must identify an atlas_bench_* disposable database")
    return args


def environment():
    root = Path(__file__).resolve().parents[1]
    return {
        "python": sys.version,
        "platform": platform.platform(),
        "logical_cpus": os.cpu_count(),
        "meminfo": Path("/proc/meminfo").read_text().splitlines()[:3],
        "packages": {d.metadata["Name"]: d.version for d in importlib.metadata.distributions()},
        "commit": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True
        ).strip(),
        "benchmark_sha256": {
            p.name: hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.joinpath("benchmarks").glob("*.py")
        },
        "transport": "ASGI in-process; process CPU/RSS includes HTTP client and validation",
        "first_request": "first per case, not cold OS/PostgreSQL cache; seeding/oracles warm data",
        "postgres_memory": "optional cgroup memory includes shared memory and file cache; not RSS",
    }


async def run(args):
    atlas = Atlas(os.environ["ATLAS_BENCH_DATABASE_URL"], log_hook=observe)
    await atlas.startup()
    report = {
        "environment": environment(),
        "arguments": vars(args) | {"output": str(args.output)},
        "config": atlas.context.config.model_dump(exclude={"database_url"}),
        "cases": [],
        "coverage": [],
        "stability": [],
        "finished": False,
    }
    cgroup = os.environ.get("ATLAS_BENCH_POSTGRES_CGROUP")
    report["environment"]["postgres_cgroup_measured"] = bool(cgroup)
    app = FastAPI()
    app.include_router(atlas.router)
    try:
        report["environment"]["postgres"] = await atlas.context.db.fetch_one(
            sql.SQL("SELECT version() AS postgres, postgis_full_version() AS postgis").format()
        )
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app), base_url="http://benchmark"
        ) as client:
            for rows in args.rows:
                for kind in ("point", "line", "polygon"):
                    async with dataset(atlas.context.db, rows, kind) as layer:
                        atlas.register(layer)
                        coverage = await mosaic(atlas.context, client, layer, MOSAIC, rows)
                        report["coverage"].append(dict(rows=rows, geometry=kind, **coverage))
                        refs = {
                            name: await reference(atlas.context, layer, tile)
                            for name, tile in (("selective", SELECTIVE), ("global", GLOBAL))
                        }
                        for repetition in range(args.repetitions):
                            for name, tile in (("selective", SELECTIVE), ("global", GLOBAL)):
                                for protocol in ("mvt", "wfs", "wms"):
                                    call = partial(
                                        request,
                                        client,
                                        protocol,
                                        layer,
                                        tile,
                                        refs[name],
                                        atlas.context.config,
                                    )

                                    for concurrency in args.concurrency:
                                        result = await measure(
                                            call,
                                            samples=args.samples,
                                            warmup=args.warmup,
                                            concurrency=concurrency,
                                            cgroup=cgroup,
                                        )
                                        ref = refs[name]
                                        result.update(
                                            rows=rows,
                                            geometry=kind,
                                            window=name,
                                            protocol=protocol,
                                            repetition=repetition + 1,
                                            matched=ref["matched"],
                                            candidates=ref["candidates"],
                                            encoded=None
                                            if ref["encoded_ids"] is None
                                            else len(ref["encoded_ids"]),
                                        )
                                        report["cases"].append(result)
                                        print(
                                            rows,
                                            kind,
                                            name,
                                            protocol,
                                            repetition + 1,
                                            concurrency,
                                            round(result["wall_seconds"], 3),
                                            flush=True,
                                        )
                        for protocol in ("mvt", "wfs", "wms"):
                            cancel_call = partial(
                                request,
                                client,
                                protocol,
                                layer,
                                GLOBAL,
                                refs["global"],
                                atlas.context.config,
                            )

                            result = await stability(
                                cancel_call, atlas.context.db, max(args.concurrency), cgroup=cgroup
                            )
                            report["stability"].append(
                                dict(rows=rows, geometry=kind, protocol=protocol, **result)
                            )
                    atlas.context.registry.remove(kind)
                    args.output.write_text(json.dumps(report, indent=2), encoding="utf-8")
        report["finished"] = True
    finally:
        await atlas.shutdown()
        args.output.write_text(json.dumps(report, indent=2), encoding="utf-8")
    bad = sum(c["outcomes"][k]["count"] for c in report["cases"] for k in ("error", "invalid"))
    pending = any(
        s["rest"][-1]["active_database_sessions"]
        or s["rest"][-1]["extra_async_tasks"]
        or s["rest"][-1]["pool"].get("requests_waiting", 0)
        for s in report["stability"]
    )
    return 1 if bad or pending else 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(run(arguments())))
