"""Compare GeoServer bodies with PostGIS oracles by content, not by serialization."""

import json
import math

from benchmarks.compare import mvt


def geojson(payload, id_column="id"):
    """Features in order as (id, geometry, properties) with the engine envelope dropped."""
    document = json.loads(payload)
    features = []
    for feature in document["features"]:
        raw = feature["id"]
        if isinstance(raw, str) and "." in raw:
            prefix, _, raw = raw.rpartition(".")
            raw = int(raw) if raw.isdigit() else f"{prefix}.{raw}"
        properties = dict(feature.get("properties") or {})
        if properties.get(id_column) == raw:
            del properties[id_column]
        features.append((raw, feature["geometry"], properties))
    return features


def close(a, b, tolerance):
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(close(x, y, tolerance) for x, y in zip(a, b, strict=True))
    if isinstance(a, int | float) and isinstance(b, int | float):
        return math.isclose(a, b, rel_tol=0, abs_tol=tolerance)
    return a == b


def difference(expected, actual, tolerance=1e-9):
    """The first kind of difference: count, ids, properties or geometry; None when equal."""
    if len(expected) != len(actual):
        return "count"
    if [f[0] for f in expected] != [f[0] for f in actual]:
        return "ids"
    if any(e[2] != a[2] for e, a in zip(expected, actual, strict=True)):
        return "properties"
    for (_, want, _), (_, got, _) in zip(expected, actual, strict=True):
        if want["type"] != got["type"] or not close(
            want["coordinates"], got["coordinates"], tolerance
        ):
            return "geometry"
    return None


def coverage(expected_tile, actual_tile):
    """Tile coverage by layer and feature id; quantized geometry is reported, not required."""
    want = {name: (extent, features) for name, extent, features in mvt(expected_tile)}
    got = {name: (extent, features) for name, extent, features in mvt(actual_tile)}
    report = {}
    for name in sorted(want.keys() | got.keys()):
        extent_a, features_a = want.get(name, (None, []))
        extent_b, features_b = got.get(name, (None, []))
        ids_a = {f[0] for f in features_a}
        ids_b = {f[0] for f in features_b}
        shapes_a = {f[0]: f[2] for f in features_a}
        shapes_b = {f[0]: f[2] for f in features_b}
        report[name] = {
            "extent": [extent_a, extent_b],
            "missing": sorted(ids_a - ids_b),
            "extra": sorted(ids_b - ids_a),
            "geometry_differs": sorted(i for i in ids_a & ids_b if shapes_a[i] != shapes_b[i]),
        }
    return report
