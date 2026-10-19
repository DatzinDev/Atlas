"""Phase 0 spike (requirements.md §34): proves the stack end to end before building the library.

Chain under test: psycopg3 async -> PostGIS -> GeoJSON, and psycopg3 -> WKB -> shapely -> Pillow.
Both served from a minimal FastAPI app.

Validated here
--------------
* psycopg3 async talks to PostGIS and reads the seeded `geo.municipios` layer.
* `ST_AsGeoJSON` builds the geometry in the database; Python only assembles the FeatureCollection.
  No coordinate arithmetic in Python, which is the project-wide invariant.
* `ST_AsBinary` + `shapely.wkb.loads` + `PIL.ImageDraw` draws recognizable polygons, lines and
  points -- the three geometry types the library has to support. This is the bet of ADR 0002
  (Pillow instead of Mapnik) and it holds for the style vocabulary of §11: fill, stroke, width,
  point radius. No system binaries needed.
* Reprojection: `geo.escuelas` is stored in EPSG:32613 and is drawn in the same image as the
  EPSG:4326 layers. The CRS change is `ST_Transform` inside the query, never arithmetic in Python.
  On the map the schools land clearly west of the municipalities, which is what a real
  reprojection looks like: mixing SRIDs without transforming would pile them on top of each other.
* North-up orientation requires flipping the Y axis: PostGIS grows northwards, image rows grow
  downwards. Getting this wrong yields a mirrored map that still "looks fine" in isolation.
* Antialiasing by drawing at 2x and downscaling with LANCZOS, as ADR 0002 anticipated: Pillow does
  not antialias polygon edges natively.

Discarded here
--------------
* `ST_AsPNG` / raster rendering in the database: it cannot honor arbitrary styles per request.
* Reprojecting in Python: any CRS change belongs in `ST_Transform`, inside the query.
* Reusing this code. It is throwaway: it lives in `examples/`, has no tests, and hardcodes what
  the library will make configurable.

Not compared here
-----------------
Mapnik was never benchmarked against this. That decision belongs to ADR 0002 and its reason is
installation cost (system binaries), not render quality -- measuring it would require installing
exactly what the ADR avoids. This spike only shows the chosen option is good enough for §11.
Polygon holes (interiors) and client-supplied BBOX/WIDTH/HEIGHT are out of scope too: they are
library design, not stack validation.

Run
---
    docker compose up -d
    pip install uvicorn        # dev-server only, deliberately not a project dependency
    python examples/spike.py
    curl -s localhost:8000/geojson | head -c 300
    curl -s localhost:8000/png -o /tmp/spike.png && file /tmp/spike.png
"""

from __future__ import annotations

import json
import os
from io import BytesIO
from typing import Any

import psycopg
from fastapi import FastAPI, Response
from PIL import Image, ImageDraw
from shapely import wkb
from shapely.geometry.base import BaseGeometry

DATABASE_URL = os.environ.get(
    "ATLAS_TEST_DATABASE_URL", "postgresql://atlas:atlas@localhost:5433/atlas"
)
IMAGE_SIZE = (800, 600)
SUPERSAMPLE = 2
BACKGROUND = (255, 255, 255)
POLYGON_FILL = (173, 216, 230)
POLYGON_OUTLINE = (30, 60, 90)
POLYGON_WIDTH = 2
LINE_COLOR = (200, 70, 40)
LINE_WIDTH = 3
POINT_COLOR = (40, 120, 60)
POINT_RADIUS = 5

# The three seeded layers, everything delivered in EPSG:4326. `geo.escuelas` is stored in
# EPSG:32613, so the CRS change happens in ST_Transform, inside the query: never in Python.
LAYER_QUERIES = (
    "SELECT ST_AsBinary(geom) FROM geo.municipios ORDER BY id",
    "SELECT ST_AsBinary(geom) FROM geo.carreteras ORDER BY id",
    "SELECT ST_AsBinary(ST_Transform(geom, 4326)) FROM geo.escuelas ORDER BY id",
)

EXTENT_QUERY = """
SELECT ST_XMin(e), ST_YMin(e), ST_XMax(e), ST_YMax(e)
FROM (
    SELECT ST_Extent(geom) AS e
    FROM (
        SELECT geom FROM geo.municipios
        UNION ALL SELECT geom FROM geo.carreteras
        UNION ALL SELECT ST_Transform(geom, 4326) FROM geo.escuelas
    ) AS all_layers
) AS extent
"""

app = FastAPI(title="Atlas spike")


@app.get("/geojson")
async def geojson() -> dict[str, Any]:
    """FeatureCollection built by PostGIS; Python only wraps the rows."""
    async with await psycopg.AsyncConnection.connect(DATABASE_URL) as conn:
        cursor = await conn.execute(
            "SELECT id, clave, nombre, poblacion, ST_AsGeoJSON(geom) "
            "FROM geo.municipios ORDER BY id"
        )
        rows = await cursor.fetchall()

    features = [
        {
            "type": "Feature",
            "id": row[0],
            "properties": {"clave": row[1], "nombre": row[2], "poblacion": row[3]},
            "geometry": json.loads(row[4]),
        }
        for row in rows
    ]
    return {"type": "FeatureCollection", "features": features}


@app.get("/png")
async def png() -> Response:
    """PNG rendered from WKB with shapely + Pillow, the bet of ADR 0002."""
    async with await psycopg.AsyncConnection.connect(DATABASE_URL) as conn:
        geometries: list[BaseGeometry] = []
        for query in LAYER_QUERIES:
            cursor = await conn.execute(query)
            geometries += [wkb.loads(row[0]) for row in await cursor.fetchall()]

        cursor = await conn.execute(EXTENT_QUERY)
        bbox = await cursor.fetchone()

    return Response(content=_render(geometries, bbox), media_type="image/png")


def _render(geometries: list[BaseGeometry], bbox: tuple[float, float, float, float]) -> bytes:
    """Draw the geometries at 2x and downscale: Pillow does not antialias polygon edges."""
    width, height = (side * SUPERSAMPLE for side in IMAGE_SIZE)
    min_x, min_y, max_x, max_y = bbox
    # A single scale for both axes keeps the map from being stretched; the leftover room is split
    # as margin. A real GetMap gets BBOX and WIDTH/HEIGHT from the client and must decide this too.
    scale = min(width / (max_x - min_x), height / (max_y - min_y))
    offset_x = (width - (max_x - min_x) * scale) / 2
    offset_y = (height - (max_y - min_y) * scale) / 2

    def to_pixels(coordinates: Any) -> list[tuple[float, float]]:
        # Y is flipped: PostGIS grows northwards, image rows grow downwards.
        return [
            (offset_x + (x - min_x) * scale, offset_y + (max_y - y) * scale) for x, y in coordinates
        ]

    image = Image.new("RGB", (width, height), BACKGROUND)
    draw = ImageDraw.Draw(image)
    for geometry in geometries:
        for part in getattr(geometry, "geoms", [geometry]):
            _draw_part(draw, part, to_pixels)

    buffer = BytesIO()
    image.resize(IMAGE_SIZE, Image.LANCZOS).save(buffer, format="PNG")
    return buffer.getvalue()


def _draw_part(draw: ImageDraw.ImageDraw, part: BaseGeometry, to_pixels: Any) -> None:
    """One branch per geometry type: the style vocabulary of §11 that ADR 0002 must cover."""
    if part.geom_type == "Polygon":
        draw.polygon(
            to_pixels(part.exterior.coords),
            fill=POLYGON_FILL,
            outline=POLYGON_OUTLINE,
            width=POLYGON_WIDTH * SUPERSAMPLE,
        )
    elif part.geom_type == "LineString":
        # joint="curve" is what keeps thick polylines from showing gaps at the vertices.
        draw.line(
            to_pixels(part.coords),
            fill=LINE_COLOR,
            width=LINE_WIDTH * SUPERSAMPLE,
            joint="curve",
        )
    elif part.geom_type == "Point":
        x, y = to_pixels(part.coords)[0]
        radius = POINT_RADIUS * SUPERSAMPLE
        draw.ellipse(
            (x - radius, y - radius, x + radius, y + radius),
            fill=POINT_COLOR,
            outline=POLYGON_OUTLINE,
        )


if __name__ == "__main__":
    try:
        import uvicorn
    except ModuleNotFoundError:  # pragma: no cover - spike only
        raise SystemExit("The spike needs a dev server: pip install uvicorn") from None

    uvicorn.run(app, host="127.0.0.1", port=8000)
