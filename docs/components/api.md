# api

> Módulo: `src/atlas/api/`

## Propósito

API REST propia, pensada para frontends (§14). Más cómoda que OGC: JSON en todo, nombres legibles,
sin XML. Es la que consume el geoportal para poblar el árbol de capas y los botones de descarga.

## Contrato público

```python
# api/layers.py
GET /layers                      -> LayerListResponse    # catálogo visible para el principal
GET /layers/{layer}              -> LayerDetailResponse  # §14 RF-LAYER-02

class LayerDetailResponse(BaseModel):
    name: str; title: str; description: str | None
    geometry_type: str; srid: int; extent: BBox | None
    attributes: list[AttributeInfo]        # name + type
    services: dict[str, bool]
    download_formats: list[str]
    style: Style | None
    min_zoom: int | None; max_zoom: int | None

# api/download.py
GET /layers/{layer}/download?format=geojson&bbox=...&poblacion__gt=100000
    -> StreamingResponse           # §15.2, filtros con la misma sintaxis que WFS

# api/maps.py
POST /maps/export                  # §16
class MapExportRequest(BaseModel):
    layers: list[str]; bbox: BBox; width: int; height: int
    crs: int = 3857
    format: Literal["png", "jpeg"] = "png"
    transparent: bool = False
    styles: dict[str, Style] | None = None    # override por capa
    -> Response(image/png | image/jpeg)
```

## Invariantes

- `GET /layers` devuelve solo capas `enabled` y sobre las que el principal tiene permiso `view`.
  Filtrar el catálogo es parte del control de acceso, no un detalle de presentación.
- Los filtros de `/download` usan **el mismo parser** que WFS (`wfs/filters.py`). No se duplica la
  lógica de whitelist en dos sitios: es la clase de duplicación que acaba en un agujero de
  seguridad en solo uno de los dos.
- `/download` requiere `layer.downloadable` y permiso `download`. Un `format` desconocido es un
  `UnsupportedFormat` 400.
- `POST /maps/export` reutiliza `wms.get_map`, con overrides de estilo locales a la petición,
  bbox siempre `minx,miny,maxx,maxy` y descarga PNG/JPEG con nombre UTC. No muta el registry.
- Todas las rutas de este paquete cuelgan del prefijo configurado en `Atlas` (por defecto `/atlas`).
- Los errores salen con la forma de `core.exceptions.error_response`, iguales que en WMS/WFS.
- Catálogo y detalle tienen modelos de respuesta explícitos en OpenAPI; el catálogo no incluye
  atributos ni nombres internos de esquema/tabla. El detalle publica atributos `{name, type}`,
  conserva la llave primaria y excluye la geometría. Una capa deshabilitada responde 404.
- `download_formats` usa el registro de formatos disponibles y queda vacío si `downloadable=False`.
- `/download` exporta en EPSG:4326, conserva la llave primaria y admite `properties` y `bbox`.
  CSV acepta `geometry=wkt|centroid|none`; `lon`/`lat` son coordenadas geográficas.
  No usa `default_limit`. Con `max_download_features` rechaza con 413 las consultas que exceden
  el máximo al comprobar el conteo, en vez de descargar silenciosamente una parte de la capa.
  El cursor consume lotes de 16 filas y se cierra incluso si el cliente falla antes de los headers.
  Los formatos de archivo completan la escritura en disco antes de enviar headers; el generador
  del exportador también se cierra explícitamente ante desconexiones para borrar el temporal.

## Dependencias

Importa: `core`, `layers`, `postgis`, `wfs` (filtros), `wms` (render de export), `exports`, `styles`.
Lo importa: `integration`.

## Fuera de alcance

- Endpoints de administración (crear/editar/borrar capas) — v0.2, §39.
- Autenticación: las rutas reciben el `principal` que inyecta la dependencia de la app host.
- Servir el frontend del geoportal. Atlas no trae UI.

## Referencias

`requirements.md` §14 (API de capas), §15 (descargas), §16 (exportación de mapas), §18 (permisos).
