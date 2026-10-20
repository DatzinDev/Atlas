# wfs

> Módulo: `src/atlas/wfs/`

## Propósito

Implementa el subconjunto de solo lectura de WFS 2.0.0 que necesita un geoportal:
`GetCapabilities`, `DescribeFeatureType` y `GetFeature` con bbox, filtros, paginación y selección
de propiedades. Salida principal: GeoJSON.

## Contrato público

```python
# wfs/router.py
def build_router(ctx: AtlasContext) -> APIRouter    # monta GET/POST /wfs, dispatch por REQUEST

# wfs/filters.py
OPERATORS: dict[str, str] = {
    "eq": "=", "ne": "!=", "gt": ">", "gte": ">=", "lt": "<", "lte": "<=",
    "in": "IN", "like": "LIKE",
}
class Filter(BaseModel):
    column: str; operator: str; value: Any

def parse_filters(params: Mapping[str, str], layer: Layer) -> list[Filter]
# "poblacion__gt=100000" -> Filter(column="poblacion", operator="gt", value=100000)
# "municipio=Guadalajara" -> Filter(column="municipio", operator="eq", value="Guadalajara")

def compile_filters(filters: list[Filter], layer: Layer) -> tuple[list[sql.Composed], list]

# wfs/get_feature.py
async def get_feature(ctx, *, type_names: str, bbox: BBox | None = None, bbox_srid: int | None = None,
                      output_srid: int = 4326, filters: list[Filter] | None = None,
                      order_by: list[tuple[str, Literal["ASC", "DESC"]]] | None = None,
                      order_by: list[tuple[str, Literal["ASC", "DESC"]]] | None = None,
                      property_names: list[str] | None = None, include_geometry: bool = True,
                      count: int | None = None, start_index: int = 0, principal=None,
                      report: OperationLog | None = None) -> StreamingResponse

# wfs/describe.py
def describe_feature_type(layer: Layer) -> dict     # JSON schema de la capa

# wfs/capabilities.py
def capabilities_xml(layers: list[Layer], base_url: str) -> str
```

## Invariantes

- **Todo filtro pasa por `parse_filters` + `compile_filters`.** Ningún otro módulo construye un
  `WHERE`. `parse_filters` rechaza toda columna que no esté en `layer.attribute_names` y todo
  operador fuera de `OPERATORS` con `InvalidFilter`.
- Los valores se castean al tipo de la columna antes de ejecutarse; un `poblacion__gt=abc` es un
  `InvalidFilter` 400, nunca un error 500 de PostgreSQL.
- `LIKE` recibe el patrón del usuario tal cual como parámetro; no se concatena `%` en el SQL.
- `compile_filters` revalida filtros creados por código y delega la composición SQL en
  `postgis.attribute_predicate`. `IN` usa una lista no vacía; los enteros respetan su ancho PostgreSQL.
- El número de features devueltos es `min(count solicitado, config.max_limit)` y por defecto
  `config.default_limit` (§23). Nunca ilimitado.
- `resolve_pagination` admite `count`/`limit` y `start_index`/`offset`; si se envían ambos nombres,
  tienen prioridad los de WFS. Los límites no positivos y offsets negativos son `InvalidFilter`.
- Se responde con `PermissionDenied` si el checker rechaza la operación `query`, y con
  `ServiceDisabled` si `layer.services.wfs` es `False`.
- La salida es `application/geo+json` en streaming; no se construye la lista completa en memoria.
- GET y POST form-encoded comparten validación; no se admite XML. Los parámetros OGC son
  case-insensitive, los nombres de atributos conservan el caso de PostgreSQL. Parámetros
  duplicados se rechazan para evitar interpretaciones ambiguas. Solo se consulta una capa por vez.
- El router registra las tres operaciones durante toda su respuesta, incluido el stream; captura
  errores previos a los headers y registra errores tardíos sin cambiar un HTTP ya iniciado.
- `get_feature` cuenta coincidencias y abre un cursor acotado. Prefetch de una fila detecta errores
  iniciales antes de enviar headers; el cursor se cierra al terminar o cancelar el iterador.
- La respuesta también cierra el cursor si el envío de headers falla antes de iniciar el cuerpo;
  la limpieza queda protegida contra la cancelación de la petición.
  `numberReturned` se escribe al final del JSON y cuenta los elementos emitidos. Un error posterior
  a los headers interrumpe el stream; no puede reemplazar una respuesta HTTP ya iniciada.
- El CRS de salida se resuelve con `core.crs.require_crs`; un CRS no soportado es 400, no 500.
- Capabilities anuncia EPSG:4326 como CRS de salida por defecto, incluso si la tabla usa UTM;
  `OtherCRS` se deriva del registro soportado. Recibe capas ya filtradas por permiso `view` y
  excluye las que no ofrecen WFS. Las URLs se toman del request, incluyendo su prefijo.
- `describe_feature_type(layer, output_format="application/json")` publica tipos JSON Schema,
  nulabilidad, campo geométrico y SRID. Tipos PostgreSQL desconocidos se describen como `string`;
  cualquier formato de salida distinto de JSON lanza `UnsupportedFormat`.
- `propertyName` admite atributos separados por comas con espacios; vacío significa todos. La
  llave primaria siempre se selecciona como identidad. La lista admite la geometría registrada:
  si no la incluye, no se ejecuta ST_AsGeoJSON y se devuelve geometry:null. Solo geometría
  conserva la PK; geometry=false prevalece incluso si se solicita la geometría explícitamente.
  Otros campos geométricos siguen fuera de la whitelist. La API Python sigue la misma selección.
- `parse_property_names(..., allow_geometry=True)` se usa solo para WFS. Su default False
  conserva las descargas como selección de atributos; pedir geometría allí sigue siendo 400.
  Compatibilidad #104: una lista WFS de solo atributos ya no devuelve geometría implícita.

## Dependencias

Importa: `core`, `postgis`, `layers`.
Lo importan: `api` (reutiliza `filters`), `integration`.

## Fuera de alcance

- `Transaction` / WFS-T, `LockFeature`, stored queries (§32).
- Filter Encoding XML (`<fes:Filter>`) — los filtros son parámetros de query (ADR 0003).
- GML como formato de salida.
- Operadores espaciales más allá de `bbox`.

## Referencias

`requirements.md` §12 (RF-WFS-01..06), §13 (filtros y seguridad), §23 (límites), §24 (streaming).
ADR 0003.

## Orden de resultados (#108)

`parse_sort_by(value, layer)` devuelve criterios `(columna, ASC|DESC)` para `get_feature(order_by=...)`.
GET y POST form aceptan `sortBy=campo A,otro D`; omitir dirección significa A.
El nombre del parámetro es case-insensitive; columnas conservan caso y direcciones son A/D.
Se rechazan columnas desconocidas/geométricas, criterios duplicados y expresiones con InvalidFilter 400.
El orden se aplica antes de COUNT/STARTINDEX y la PK desempata automáticamente si no se solicitó.
Las vistas sin PK requieren identidad única del host para garantizar páginas estables; modificaciones
entre peticiones no comparten snapshot. No se anuncia FES SortBy XML.
