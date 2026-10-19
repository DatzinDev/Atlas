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
async def get_feature(ctx, *, type_names: str, bbox: BBox | None, bbox_crs: int,
                      output_crs: int, filters: list[Filter], property_names: list[str] | None,
                      count: int | None, start_index: int) -> FeatureCollection

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
- `get_feature` cuenta coincidencias y abre un cursor acotado. Prefetch de una fila detecta errores
  iniciales antes de enviar headers; el cursor se cierra al terminar o cancelar el iterador.
  `numberReturned` se escribe al final del JSON y cuenta los elementos emitidos. Un error posterior
  a los headers interrumpe el stream; no puede reemplazar una respuesta HTTP ya iniciada.
- El CRS de salida se resuelve con `core.crs.require_crs`; un CRS no soportado es 400, no 500.
- `propertyName` admite atributos separados por comas con espacios; vacío significa todos. La
  llave primaria siempre se selecciona. `geometry=false` evita consultar la geometría y produce
  `geometry: null` para conservar la estructura GeoJSON del feature.

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
