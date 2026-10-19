# wms

> Módulo: `src/atlas/wms/`

## Propósito

Implementa `GetCapabilities`, `GetMap` y `GetFeatureInfo` de WMS 1.3.0. Traduce parámetros WMS en
consultas PostGIS y delega el dibujo a `rendering/`.

## Contrato público

```python
# wms/router.py
def build_router(ctx: AtlasContext) -> APIRouter    # GET /wms, dispatch por REQUEST

# wms/get_map.py
class GetMapParams(BaseModel):
    layers: list[str]; bbox: BBox; width: int; height: int; crs: int
    format: Literal["image/png", "image/jpeg"] = "image/png"
    styles: list[str] = []
    transparent: bool = False
    bgcolor: str = "#FFFFFF"

    @field_validator("width", "height")   # 1..MAX_IMAGE_SIZE
async def get_map(ctx: AtlasContext, params: GetMapParams, principal: Any = None) -> Response

# wms/feature_info.py
class FeatureInfoParams(BaseModel):
    query_layers: list[str]; i: int; j: int; feature_count: int = 1
    info_format: Literal["application/json"] = "application/json"
    # + los mismos campos de GetMapParams necesarios para georreferenciar el píxel
async def get_feature_info(ctx, params, principal) -> dict

# wms/capabilities.py
def capabilities_xml(layers: list[Layer], base_url: str) -> str
```

## Invariantes

- `WIDTH` y `HEIGHT` están acotados por una constante de configuración. Una petición de
  20000×20000 es un 400, no un OOM.
- Las capas se dibujan en el orden en que llegan en `LAYERS`: la primera al fondo, la última
  arriba (orden WMS, no el inverso).
- El bbox se transforma al SRID de cada tabla para filtrar (índice GIST), y las geometrías se
  transforman al CRS de salida en el `SELECT`. Nunca al revés.
- En EPSG:4326 con WMS 1.3.0 el orden de ejes del BBOX es **lat,lon**. Se normaliza en el parseo,
  antes de tocar nada más, y hay un test que lo fija.
- `GetFeatureInfo` convierte el píxel `(i, j)` en un punto del mundo y consulta con un buffer de
  tolerancia en píxeles, no en unidades del CRS.
- `image/jpeg` ignora `TRANSPARENT=TRUE` (JPEG no tiene alfa): se compone sobre `BGCOLOR`.
- `GetMapParams` recibe el bbox en el orden del protocolo y lo normaliza una sola vez al
  construir el modelo. `CRS:84` conserva ejes longitud/latitud.
- `GetMap` comprueba permisos de todas las capas antes de consultar, usa los límites de features
  por capa de `AtlasConfig` y ejecuta el renderer en un thread para no bloquear el event loop.
  Los nombres de `STYLES` se aceptan sin sustituir el estilo configurado en la capa.
- `wms/` no dibuja nada por sí mismo: llama a `MapRenderer.render()`.
- Se comprueba el permiso `view` para `GetMap` y `query` para `GetFeatureInfo`, por capa.

## Dependencias

Importa: `core`, `postgis`, `layers`, `styles`, `rendering`.
Lo importan: `api` (para `/maps/export`), `integration`.

## Fuera de alcance

- `GetLegendGraphic`, `DescribeLayer`, SLD (ADR 0003).
- WMS 1.0/1.1 (el orden de ejes cambia; no se soportan).
- Tiles y caché — WMTS/MVT son v0.3.
- El algoritmo de dibujo: vive en `rendering/`.

## Referencias

`requirements.md` §9 (RF-WMS-01..03), §10 (renderizado), §11 (estilos), §16 (exportación de mapas),
§20 (CRS). ADR 0002, ADR 0003.
