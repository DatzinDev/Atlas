"""Correctness oracles and request classification, independent of timing summaries."""

import hashlib
from contextvars import ContextVar
from io import BytesIO
from time import perf_counter

import mapbox_vector_tile
from PIL import Image
from psycopg import sql
from shapely.geometry import shape

from atlas.core.tiles import tile_bounds
from atlas.postgis.queries import bbox_predicate

OBSERVED = ContextVar("benchmark_feature_counts", default=None)


def observe(report):
    counts = OBSERVED.get()
    if counts is not None:
        counts.append(report.feature_count)


async def reference(ctx, layer, tile):
    """Independently count all candidates; quantization is checked only below the cap."""
    bounds = tile_bounds(tile)
    margin = (bounds[2] - bounds[0]) * ctx.config.mvt_buffer / ctx.config.mvt_extent
    expanded = (bounds[0] - margin, bounds[1] - margin, bounds[2] + margin, bounds[3] + margin)
    relation = sql.Identifier(layer.schema_name, layer.table_name)
    bbox, params = bbox_predicate(layer.table_info, bounds, 3857)
    matched = await ctx.db.fetch_one(
        sql.SQL("SELECT count(*) AS n FROM {} WHERE {}").format(relation, bbox), params
    )
    page = await ctx.db.fetch_all(
        sql.SQL("SELECT id FROM {} WHERE {} ORDER BY id LIMIT %s").format(relation, bbox),
        [*params, ctx.config.default_limit],
    )
    candidate, values = bbox_predicate(layer.table_info, expanded, 3857)
    total = await ctx.db.fetch_one(
        sql.SQL("SELECT count(*) AS n FROM {} WHERE {}").format(relation, candidate), values
    )
    encoded = None
    if total["n"] <= ctx.config.max_tile_features:
        rows = await ctx.db.fetch_all(
            sql.SQL("""
            SELECT id FROM {table} WHERE {bbox} AND ST_AsMVTGeom(
                geom, ST_MakeEnvelope(%s,%s,%s,%s,3857)::box2d, %s,%s,true) IS NOT NULL
        """).format(table=relation, bbox=candidate),
            [*values, *bounds, ctx.config.mvt_extent, ctx.config.mvt_buffer],
        )
        encoded = {r["id"] for r in rows}
    return {
        "matched": matched["n"],
        "page_ids": [r["id"] for r in page],
        "candidates": total["n"],
        "encoded_ids": encoded,
    }


def validate(response, protocol, name, expected, config, counts=()):
    """Return a semantic outcome; incomplete data never increments complete success."""
    if response.status_code != 200:
        return "rejected" if response.status_code == 413 else "error"
    if protocol == "mvt":
        decoded = mapbox_vector_tile.decode(response.content) if response.content else {}
        if expected["encoded_ids"] is None:
            raise AssertionError("oversized tile returned success")
        features = decoded.get(name, {}).get("features", [])
        ids = [f["id"] for f in features]
        assert len(ids) == len(set(ids)) and set(ids) == expected["encoded_ids"]
        assert all(f["id"] == f["properties"]["id"] for f in features)
        for feature in features:
            assert all(
                -config.mvt_buffer <= v <= config.mvt_extent + config.mvt_buffer
                for v in shape(feature["geometry"]).bounds
            )
        return "complete"
    if protocol == "wfs":
        data = response.json()
        assert data["numberMatched"] == expected["matched"]
        assert data["numberReturned"] == len(expected["page_ids"])
        assert [f["id"] for f in data["features"]] == expected["page_ids"]
        return "complete" if data["numberReturned"] == data["numberMatched"] else "page"
    image = Image.open(BytesIO(response.content))
    image.load()
    assert image.size == (256, 256) and image.format == "PNG"
    assert counts and counts[-1] == min(expected["matched"], config.default_limit)
    return "complete" if counts[-1] == expected["matched"] else "incomplete"


async def request(client, protocol, layer, tile, expected, config):
    bounds = ",".join(map(str, tile_bounds(tile)))
    path, params = {
        "mvt": (f"/atlas/tiles/{layer.name}/{tile.z}/{tile.x}/{tile.y}.mvt", {}),
        "wfs": (
            "/atlas/wfs",
            dict(
                REQUEST="GetFeature",
                TYPENAMES=layer.name,
                BBOX=bounds + ",3857",
                SRSNAME="EPSG:3857",
                COUNT=config.default_limit,
            ),
        ),
        "wms": (
            "/atlas/wms",
            dict(
                REQUEST="GetMap",
                LAYERS=layer.name,
                CRS="EPSG:3857",
                BBOX=bounds,
                WIDTH=256,
                HEIGHT=256,
            ),
        ),
    }[protocol]
    token = OBSERVED.set([])
    try:
        start = perf_counter()
        response = await client.get(path, params=params)
        elapsed = (perf_counter() - start) * 1000
        try:
            digest = hashlib.sha256(response.content).digest()
            if (
                protocol == "mvt"
                and response.status_code == 200
                and expected.get("validated_mvt_sha256") == digest
            ):
                outcome = "complete"
            else:
                outcome = validate(response, protocol, layer.name, expected, config, OBSERVED.get())
                if protocol == "mvt" and outcome == "complete":
                    # Repeated decoding would distort CPU and concurrent request latency.
                    expected["validated_mvt_sha256"] = digest
        except Exception:  # Decoder/image failures are integrity failures, never fast successes.
            outcome = "invalid"
        return {
            "ms": elapsed,
            "bytes": len(response.content),
            "status": response.status_code,
            "outcome": outcome,
        }
    finally:
        OBSERVED.reset(token)


async def mosaic(ctx, client, layer, tiles, expected_rows):
    covered = set()
    candidates = coded = 0
    for tile in tiles:
        expected = await reference(ctx, layer, tile)
        response = await client.get(f"/atlas/tiles/{layer.name}/{tile.z}/{tile.x}/{tile.y}.mvt")
        assert validate(response, "mvt", layer.name, expected, ctx.config) == "complete"
        assert expected["encoded_ids"] is not None
        covered.update(expected["encoded_ids"])
        candidates += expected["candidates"]
        coded += len(expected["encoded_ids"])
    assert covered == set(range(1, expected_rows + 1)), "mosaic lost dataset IDs"
    return {
        "unique_ids": len(covered),
        "candidates_with_buffer": candidates,
        "encoded_with_buffer": coded,
        "duplicates_with_buffer": coded - len(covered),
    }
