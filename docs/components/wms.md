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
class FeatureInfoParams(GetMapParams):
    query_layers: list[str]; i: int; j: int; feature_count: int = 1
    info_format: Literal["application/json"] = "application/json"
    tolerance_pixels: float = 5
    include_geometry: bool = False
async def get_feature_info(ctx, params, principal) -> dict

# wms/capabilities.py
def capabilities_xml(layers: list[Layer], base_url: str, *, title: str = "Atlas WMS") -> str
```

## Invariantes

- `capabilities_xml` recibe capas ya filtradas por permiso. Omite las capas deshabilitadas
  o sin WMS y serializa metadatos con ElementTree. El extent geográfico alimenta EPSG:4326
  (con ejes lat/lon); para EPSG:3857 anuncia límites globales conservadores del registro de CRS,
  ya que el metadato no guarda extents proyectados y el serializer no consulta PostGIS.
  Sin extent se omiten los bounding boxes.
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
- `GetFeatureInfo` utiliza el centro del píxel y una ventana de tolerancia escalada por
  separado en X/Y. PostGIS descarta candidatos que no intersectan realmente la ventana
  (incluidos huecos de polígonos). Devuelve GeoJSON con `layer` por elemento; la geometría
  es nula por defecto y, si se pide, sale en EPSG:4326. `FEATURE_COUNT` se acota por capa.
- `image/jpeg` ignora `TRANSPARENT=TRUE` (JPEG no tiene alfa): se compone sobre `BGCOLOR`.
- `GetMapParams` recibe el bbox en el orden del protocolo y lo normaliza una sola vez al
  construir el modelo. `CRS:84` conserva ejes longitud/latitud.
- `GetMap` comprueba permisos de todas las capas antes de consultar, usa un presupuesto de features/WKB
  por mapa de `AtlasConfig` y ejecuta el renderer en un thread para no bloquear el event loop.
  `STYLES` omitido/vacío o con vacíos por capa usa el estilo registrado; nombres no vacíos
  se rechazan antes de consultar/renderizar. Los overrides tipados de `/maps/export` no cambian.
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

## Compatibilidad — corrección #102

Antes se ignoraban nombres de estilos y parámetros de contenido. GetMap/GetFeatureInfo
ahora rechazan con InvalidParameter (400) CQL_FILTER, TIME, ENV, SLD, SLD_BODY, FILTER,
ATLAS_FILTER, incluso vacíos y sin distinguir mayúsculas. No pasan al
pipeline de datos/render/caché. Los parámetros de transporte inocuos siguen tolerándose;
TILED no activa un caché ni se interpreta como filtro. No se anuncian capacidades nuevas.

## Filtros compartidos por capa (#125)

`ATLAS_FILTERS` acepta un objeto JSON `{nombre_capa: predicado}` con el contrato
compartido del ADR 0005. Las claves pertenecen a `LAYERS`; bytes, nodos y coordenadas
se acotan por petición y la profundidad por árbol mediante `AtlasConfig`.
El modelo Python expone `atlas_filters`; los servicios vuelven a comprobar los límites
configurados. GetMap compila cada filtro con la metadata de su capa y lo combina con el bbox mediante AND.
Todos los permisos y columnas se comprueban antes de la primera consulta.
GetFeatureInfo reutiliza el mismo compilador por `QUERY_LAYERS` y combina el filtro
con la intersección exacta de la ventana del clic. `{}` conserva el comportamiento anterior.
No existe caché en la base integrada: cada solicitud ejecuta su filtro de forma independiente. CQL y ATLAS_FILTER siguen rechazados.


## Mapas completos y presupuestos (#193)

GetMap y `/maps/export` comparten `max_render_features=100000` y
`max_render_bytes=67108864` acumulados entre todas las capas, independientes de WFS.
Se devuelve el mapa completo bajo el contrato del renderer o QueryTooLarge 413 antes
de dibujar; nunca un subconjunto silencioso. Solicitar una ventana menor o filtros más
selectivos permite reducir el trabajo. No se simplifica ni se modifica el dataset.
La paginación WFS y los límites de imagen se conservan. Los bytes miden WKB de entrada;
no prometen un techo exacto de RSS. Geometrías inválidas/vacías conservan el contrato
preexistente del renderer. El benchmark distingue mapa completo, rechazo y respuesta incompleta.
