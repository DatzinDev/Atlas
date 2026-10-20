# Arquitectura

Atlas es una librería, no un servicio. Vive dentro del proceso de la aplicación FastAPI del
usuario y traduce peticiones OGC/HTTP en consultas PostGIS.

## Capas

```text
                    Aplicación FastAPI del usuario
                                │
                        atlas.Atlas.router
                                │
        ┌───────────────┬───────┴────────┬───────────────┐
        │               │                │               │
     api/            wms/             wfs/          exports/
   (REST propio)   (GetMap,        (GetFeature,    (GeoJSON, CSV,
                   GetFeatureInfo)  Describe...)    GPKG, SHP)
        │               │                │               │
        └───────────────┴───────┬────────┴───────────────┘
                                │
                    layers/  (Layer, LayerRegistry, permisos)
                                │
        ┌───────────────────────┼───────────────────────┐
        │                       │                       │
    postgis/                rendering/               styles/
 (pool, SQL,              (MapRenderer,            (modelos de
  introspección)           PillowRenderer)          estilo)
        │                       │
        └───────────────────────┘
                    │
                 PostGIS
```

## Reglas de dependencia

La flecha va siempre hacia abajo. Un componente **nunca** importa de una capa superior.

| Componente | Puede importar | Nunca importa |
|---|---|---|
| `core` | — | nada del proyecto |
| `postgis` | `core` | `layers`, `wms`, `wfs`, `api` |
| `styles` | `core` | todo lo demás |
| `rendering` | `core`, `styles` | `postgis`, `wms`, `layers` |
| `layers` | `core`, `postgis` | `wms`, `wfs`, `api`, `exports` |
| `wfs` | `core`, `postgis`, `layers` | `wms`, `api`, `rendering` |
| `wms` | `core`, `postgis`, `layers`, `styles`, `rendering` | `wfs`, `api` |
| `exports` | `core`, `postgis`, `layers` | `wms`, `wfs`, `api` |
| `api` | todos los anteriores | — |
| `integration` | todos | — |

`rendering` no habla con PostGIS: recibe geometrías ya consultadas y proyectadas. Eso es lo que
permite sustituir Pillow por Mapnik sin tocar `wms/`.

## Flujo de una petición WMS GetMap

```text
GET /wms?REQUEST=GetMap&LAYERS=municipios&BBOX=...&WIDTH=800&HEIGHT=600&CRS=EPSG:3857
   │
   ├─ wms/router.py        valida parámetros, resuelve REQUEST
   ├─ layers/registry.py   busca la capa, verifica wms_enabled y permiso view
   ├─ postgis/queries.py   SELECT ST_AsBinary(ST_Transform(geom, 3857)) WHERE geom && bbox
   ├─ styles/models.py     resuelve el estilo activo de la capa
   ├─ rendering/pillow.py  proyecta WKB a píxeles y dibuja
   └─ Response(image/png)
```

## Flujo de una petición WFS GetFeature

```text
GET /wfs?REQUEST=GetFeature&TYPENAMES=municipios&bbox=...&poblacion__gt=100000&limit=500
   │
   ├─ wfs/router.py        resuelve REQUEST
   ├─ layers/registry.py   busca la capa, verifica wfs_enabled y permiso query
   ├─ wfs/filters.py       parsea filtros contra whitelist de columnas y operadores
   ├─ postgis/queries.py   compone SQL parametrizado con psycopg.sql
   └─ StreamingResponse(application/geo+json)
```

## Qué no existe en esta arquitectura

- No hay proceso, daemon ni servidor propio.
- No hay tabla de usuarios ni middleware de autenticación (§17 de `requirements.md`).
- No hay motor de reproyección propio: vectores con `ST_Transform`; el futuro extra ráster usará GDAL.
- No hay caché en v0.1 — pero ningún componente guarda estado mutable que impida añadirla después.

## Extensiones aprobadas, aún sin implementar (#100)

MVT se generará en `postgis/` y se autorizará en `tiles/`; no usa el renderer WMS.
El extra ráster local tendrá `RasterSource` y registro separados de `Layer` PostGIS.
El núcleo y la importación de Atlas no cargarán Rasterio. PostGIS sigue siendo requisito.
Las políticas de producto, OIDC, calendarios y adaptadores quedan en la aplicación.
Los ADRs de consultas, MVT y ráster fijarán las nuevas dependencias antes de implementarlas.

### Consultas compartidas (propuesta)

[ADR 0005](adr/0005-shared-query-predicates.md): core define predicados; postgis
compila con TableInfo. WFS/WMS/tiles/exports autorizan y adaptan sin importar entre sí.
Los filtros fijos y de usuario se combinan con AND; paginación no limita mapas completos.

### Tiles vectoriales (propuesta)

[ADR 0006](adr/0006-native-mvt.md): tiles → core/layers/postgis; postgis → core.
Grid compartido en core; HTTP y autorización en tiles; SQL y codificación en postgis.
La integración monta el router y delega en get_vector_tile sin WMS, WFS ni rendering.
