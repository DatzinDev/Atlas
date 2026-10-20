"""Fixtures are deterministic, indexed, isolated and cleaned after failure."""

from unittest.mock import AsyncMock

import pytest
from benchmarks.vector_data import dataset
from psycopg import sql


async def test_reject_application_database_before_ddl():
    db = AsyncMock()
    db.fetch_one.return_value = {"name": "production"}
    with pytest.raises(ValueError, match="refusing DDL"):
        async with dataset(db, 10, "point"):
            pytest.fail("unsafe database accepted")
    db.connection.assert_not_called()


@pytest.mark.parametrize("kind", ["point", "line", "polygon"])
async def test_deterministic_fixture_and_cleanup(bench_db, kind):
    snapshots = []
    for _ in range(2):
        with pytest.raises(RuntimeError, match="intentional"):
            async with dataset(bench_db, 16, kind) as layer:
                relation = sql.Identifier(layer.schema_name, layer.table_name)
                rows = await bench_db.fetch_all(
                    sql.SQL("SELECT id, ST_AsEWKT(geom) AS geometry FROM {} ORDER BY id").format(
                        relation
                    )
                )
                assert len(rows) == 16 and layer.services.mvt
                snapshots.append(rows)
                assert await bench_db.fetch_one(
                    sql.SQL("SELECT count(*) AS n FROM pg_indexes WHERE schemaname=%s").format(),
                    [layer.schema_name],
                ) == {"n": 2}
                schema = layer.schema_name
                raise RuntimeError("intentional")
        assert await bench_db.fetch_one(
            sql.SQL("SELECT count(*) AS n FROM pg_namespace WHERE nspname=%s").format(), [schema]
        ) == {"n": 0}
    assert snapshots[0] == snapshots[1]
