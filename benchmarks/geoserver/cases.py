"""Translate canonical Atlas cases into GeoServer WFS/WMS requests; never measures anything."""

import argparse
import json
import math
import sys
from pathlib import Path
from urllib.parse import urlencode

from benchmarks.geoserver.setup import WORKSPACE

ORIGIN = 20037508.342789244
TILE_PIXELS = 256
EXTENT = 4096
BUFFER = 64
NOT_APPLICABLE = {
    "catalog": "GeoServer has no equivalent of the Atlas dataset catalog",
    "tilejson": "GeoServer WMS does not publish TileJSON for a layer",
}
WFS_CASES = ("geojson-page", "geojson-query", "export")
COMPARISONS = {
    "eq": "=",
    "ne": "<>",
    "gt": ">",
    "gte": ">=",
    "lt": "<",
    "lte": "<=",
    "like": "LIKE",
    "ilike": "ILIKE",
}
QUERY_KEYS = {"filter", "bbox", "properties", "sort", "limit", "offset"}


def field(name):
    if not isinstance(name, str) or not name or '"' in name:
        raise ValueError(f"unsupported field name {name!r}")
    return f'"{name}"'


def literal(value):
    if isinstance(value, bool):
        return "TRUE" if value else "FALSE"
    if isinstance(value, int | float):
        if not math.isfinite(value):
            raise ValueError("non-finite number")
        return json.dumps(value)
    if isinstance(value, str):
        return "'" + value.replace("'", "''") + "'"
    raise ValueError(f"unsupported value {value!r}")


def wkt(geometry):
    def ring(points):
        return "(" + ", ".join(f"{json.dumps(x)} {json.dumps(y)}" for x, y in points) + ")"

    def polygon(rings):
        return "(" + ", ".join(ring(r) for r in rings) + ")"

    kind, coordinates = geometry["type"], geometry["coordinates"]
    if kind == "Polygon":
        return "SRID=4326;POLYGON" + polygon(coordinates)
    if kind == "MultiPolygon":
        return "SRID=4326;MULTIPOLYGON(" + ", ".join(polygon(p) for p in coordinates) + ")"
    raise ValueError(f"unsupported geometry {kind!r}")


def cql(node, geometry):
    """The Atlas filter AST as ECQL; any operator outside the contract is an error."""
    op = node["op"]
    if op in ("and", "or"):
        return "(" + f" {op.upper()} ".join(cql(arg, geometry) for arg in node["args"]) + ")"
    if op == "not":
        return f"(NOT {cql(node['arg'], geometry)})"
    if op == "in":
        values = ", ".join(literal(v) for v in node["value"])
        return f"{field(node['field'])} IN ({values})"
    if op in ("is_null", "is_not_null"):
        return f"{field(node['field'])} IS {'NOT ' if op == 'is_not_null' else ''}NULL"
    if op in ("intersects", "within"):
        return f"{op.upper()}({geometry}, {wkt(node['geometry'])})"
    if op in COMPARISONS:
        return f"{field(node['field'])} {COMPARISONS[op]} {literal(node['value'])}"
    raise ValueError(f"unsupported operator {op!r}")


def wfs(base, layer, query, case, columns, default_limit):
    geometry, key = columns
    if unknown := set(query) - QUERY_KEYS:
        raise ValueError(f"unknown query keys {sorted(unknown)}")
    clauses = []
    if "bbox" in query:
        minx, miny, maxx, maxy = (literal(v) for v in query["bbox"])
        clauses.append(f"BBOX({geometry}, {minx}, {miny}, {maxx}, {maxy}, 'EPSG:4326')")
    if "filter" in query:
        clauses.append(cql(query["filter"], geometry))
    params = {
        "service": "WFS",
        "version": "2.0.0",
        "request": "GetFeature",
        "typeNames": f"{WORKSPACE}:{layer}",
        "outputFormat": "application/json",
        "srsName": "EPSG:4326",
    }
    if clauses:
        params["CQL_FILTER"] = " AND ".join(clauses)
    if "properties" in query:
        params["propertyName"] = ",".join([*query["properties"], geometry])
    order = []
    for item in query.get("sort", []):
        name, _, direction = item.partition(":")
        if direction not in ("", "asc", "desc"):
            raise ValueError(f"unsupported sort {item!r}")
        order.append((name, "D" if direction == "desc" else "A"))
    if key not in (name for name, _ in order):
        order.append((key, "A"))
    if case == "export":
        if {"limit", "offset", "sort"} & set(query):
            raise ValueError("export accepts only filter, bbox and properties")
    else:
        params["count"] = str(query.get("limit", default_limit))
        params["startIndex"] = str(query.get("offset", 0))
    params["sortBy"] = ",".join(f"{name} {d}" for name, d in order)
    return f"{base.rstrip('/')}/{WORKSPACE}/ows?{urlencode(params)}"


def tile(base, layer, z, x, y):
    if not (0 <= z <= 30 and 0 <= x < 2**z and 0 <= y < 2**z):
        raise ValueError("tile out of range")
    size = 2 * ORIGIN / 2**z
    minx, maxy = -ORIGIN + x * size, ORIGIN - y * size
    params = {
        "service": "WMS",
        "version": "1.3.0",
        "request": "GetMap",
        "layers": f"{WORKSPACE}:{layer}",
        "styles": "",
        "crs": "EPSG:3857",
        "bbox": ",".join(repr(v) for v in (minx, maxy - size, minx + size, maxy)),
        "width": str(TILE_PIXELS),
        "height": str(TILE_PIXELS),
        "buffer": str(BUFFER * TILE_PIXELS // EXTENT),
        "format": "application/vnd.mapbox-vector-tile",
    }
    return f"{base.rstrip('/')}/{WORKSPACE}/wms?{urlencode(params)}"


def request(args):
    if args.case in NOT_APPLICABLE:
        return {"status": "n/a", "reason": NOT_APPLICABLE[args.case]}
    if args.case == "mvt-tile":
        z, x, y = (int(v) for v in args.tile.split("/"))
        return {"status": "ok", "kind": "mvt", "url": tile(args.url, args.layer, z, x, y)}
    query = json.loads(args.query.read_text()) if args.query else {}
    columns = (args.geometry, args.id)
    url = wfs(args.url, args.layer, query, args.case, columns, args.default_limit)
    return {"status": "ok", "kind": "json", "url": url}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", required=True, choices=[*NOT_APPLICABLE, *WFS_CASES, "mvt-tile"])
    parser.add_argument("--url", default="http://127.0.0.1:8081/geoserver")
    parser.add_argument("--layer", required=True)
    parser.add_argument("--geometry", default="geom")
    parser.add_argument("--id", default="id", help="id column, the final sort key")
    parser.add_argument("--query", type=Path, help="Atlas Query JSON")
    parser.add_argument("--tile", help="z/x/y for mvt-tile")
    parser.add_argument("--default-limit", type=int, default=1000)
    args = parser.parse_args(argv)
    if (args.case == "mvt-tile") != (args.tile is not None):
        parser.error("--tile is required for mvt-tile and only there")
    try:
        result = request(args)
    except (KeyError, TypeError, ValueError) as error:
        parser.error(f"cannot translate {args.case}: {error}")
    print(json.dumps({"case": args.case, "engine": "geoserver", **result}, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
