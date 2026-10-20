# postgis

> Módulo: `src/atlas/postgis/`

## Propósito

Único punto del proyecto que habla con PostgreSQL. Gestiona el pool, descubre metadatos de tablas
y vistas, y compone el SQL espacial parametrizado que consumen `wfs/`, `wms/` y `exports/`.

## Contrato público

```python
# postgis/connection.py
class Database:
    def __init__(self, config: AtlasConfig) -> None
    pool: AsyncConnectionPool | None        # None mientras está cerrada
    async def open(self) -> None
    async def close(self) -> None
    @asynccontextmanager
    def connection(self) -> AsyncIterator[AsyncConnection]
    async def fetch_all(self, query: sql.Composed, params: Sequence | None) -> list[dict]
    async def fetch_one(self, query: sql.Composed, params: Sequence | None) -> dict | None
    async def stream(self, query, params, batch_size=1000) -> AsyncIterator[dict]  # server-side cursor

# postgis/introspection.py
class ColumnInfo(BaseModel):
    name: str; data_type: str; nullable: bool

class TableInfo(BaseModel):
    schema_name: str; table_name: str
    geometry_column: str; geometry_type: GeometryType; srid: int
    columns: list[ColumnInfo]          # NO incluye la columna de geometría
    primary_key: str | None
    is_view: bool

async def inspect_table(db: Database, schema: str, table: str) -> TableInfo
def parse_source(source: str, default_schema: str = "public") -> tuple[str, str]
async def table_extent(db: Database, info: TableInfo, srid: int = 4326) -> BBox | None

# postgis/queries.py
def build_feature_query(
    info: TableInfo, *, columns: Sequence[str] | None = None,
    where: Sequence[sql.Composed] = (), params: Sequence = (),
    geometry_as: Literal["geojson", "wkb", "none"] = "geojson",
    target_srid: int | None = None, limit: int | None = None, offset: int = 0,
    order_by: Sequence[tuple[str, Literal["ASC", "DESC"]]] = (),
) -> tuple[sql.Composed, list]

def bbox_predicate(info: TableInfo, bbox: BBox, bbox_srid: int) -> tuple[sql.Composed, list]
def intersects_bbox_predicate(info: TableInfo, bbox: BBox, bbox_srid: int) -> tuple[sql.Composed, list]
def parse_bbox(value: str) -> tuple[BBox, int | None]
def count_query(info: TableInfo, where, params) -> tuple[sql.Composed, list]
async def fetch_render_geometries(
    db: Database, info: TableInfo, *, bbox: BBox, bbox_srid: int, target_srid: int,
    where: Sequence[sql.Composed] = (), params: Sequence = (), limit: int | None = None,
) -> list[BaseGeometry]

# postgis/geometry.py
def to_geojson_feature(row: dict, id_column: str | None) -> dict
```

## Invariantes

- **Ningún f-string, `%` ni `.format()` de Python sobre SQL.** Identificadores con
  `psycopg.sql.Identifier`, valores con placeholders `%s`.
- `build_feature_query` valida cada nombre de columna contra `info.columns` y lanza `InvalidFilter`
  si no está. No confía en su llamador.
- La geometría seleccionada conserva su nombre de columna; `to_geojson_feature` acepta ese nombre
  como tercer argumento (por defecto `geom`). La llave primaria se conserva en propiedades e `id`.
- Los predicados espaciales dejan la columna de geometría desnuda a la izquierda
  (`geom && ST_MakeEnvelope(...)`), nunca `ST_Transform(geom, X) && ...`, para no perder el índice
  GIST (§22). Se transforma el bbox al SRID de la tabla, no la tabla al del bbox.
- Bbox usa orden `minx,miny,maxx,maxy` en la API GeoJSON, incluso para EPSG:4326. Se rechazan
  límites invertidos y valores no finitos. El CRS opcional se valida con el registro compartido.
- La reproyección de salida (`ST_Transform` sobre el SELECT) es lo único que sí envuelve la
  geometría, y va después del filtrado.
- `fetch_render_geometries` aplica `default_limit` / `max_limit` de la configuración del pool,
  filtra con el bbox en el SRID nativo y transforma solo el SELECT. Devuelve Shapely desde WKB
  y descarta geometrías nulas; no simplifica ni recorta las geometrías.
- `build_feature_query` acepta orden por atributos introspectados con ASC/DESC explícitos;
  rechaza geometría, criterios duplicados y direcciones desconocidas. Añade PK simple ASC por
  defecto y como desempate si no aparece explícitamente. No altera la lista del llamador.
  Vistas/tablas sin PK no tienen orden total garantizado: el host debe publicar un identificador
  único. La paginación OFFSET es reproducible con datos estáticos, no un snapshot entre requests.
- `stream()` usa cursor con nombre. Ninguna consulta de descarga materializa el dataset completo.
- `inspect_table` funciona igual para tablas y para vistas; si `geometry_columns` no resuelve, cae
  a inspeccionar el tipo real de la columna.
- Se elige la primera columna `geometry` por orden de declaración y se excluyen todas las
  geometrías de `columns`. `data_type` conserva el nombre de tipo PostgreSQL (`int4`, `text`, etc.).
- Las geometrías genéricas se deducen de la primera fila no nula. Sin metadatos concretos ni una
  fila disponible se lanza `InvalidGeometry`; una tabla vacía con geometría tipada sí funciona.
  La muestra no garantiza homogeneidad geométrica en columnas sin restricciones.
- Una llave primaria compuesta se informa como `None`: no hay una sola columna identificadora.
- `table_extent` devuelve `(minx, miny, maxx, maxy)` en el SRID solicitado (4326 por defecto).
  Usa `ST_EstimatedExtent` con estadísticas; para vistas o estadísticas ausentes usa `ST_Extent`
  sobre las geometrías transformadas. Un conjunto vacío, nulo o de geometrías vacías devuelve
  `None`, incluso con estadísticas antiguas. Los SRID no positivos son `InvalidParameter`.
  El extent estimado es aproximado y depende de la vigencia de las estadísticas de PostgreSQL;
  no garantiza contener todos los elementos. No se recalcula ni se cachea en Atlas.
- `parse_source` acepta `tabla` o `esquema.tabla`, sin sintaxis SQL de identificadores entrecomillados;
  los nombres conservan mayúsculas. Un nombre vacío o con más de un punto es `InvalidParameter`.
- El `query_timeout` de `AtlasConfig` se aplica como `statement_timeout` en la conexión.
- `open()` y `close()` son idempotentes: abrir dos veces no crea dos pools y cerrar dos veces no
  falla. El ciclo de vida del pool lo marca la app host, y puede repetir la llamada.
- **Ninguna excepción de psycopg sale de `postgis/`**, incluida la espera por un pool saturado
  (`psycopg_pool.PoolTimeout` hereda de `psycopg.Error`). Una consulta cancelada por el
  `statement_timeout` es `QueryTimeout`; cualquier otro fallo del driver es `DatabaseError`, con el
  mensaje real en `technical_message` y fuera de la respuesta.

## Dependencias

Importa: `core`.
Lo importan: `layers`, `wfs`, `wms`, `exports`.

## Fuera de alcance

- Conocer qué es una `Layer` — recibe `TableInfo`, no capas.
- Serializar respuestas HTTP.
- Cualquier otro motor de base de datos (§2.2).

## Referencias

`requirements.md` §13.1 (seguridad), §19 (introspección), §20 (CRS), §21 (bbox), §22 (rendimiento),
§24 (streaming). ADR 0001.

## Predicados compartidos de atributos (#112)

`compile_predicate(predicate: Predicate | None, info: TableInfo)` devuelve `(sql.Composed, list)`.
Recibe el árbol previamente validado por `core.validate_predicate`; None devuelve SQL vacío sin WHERE.
Compone AND/OR/NOT con paréntesis, comparaciones, IN, LIKE/ILIKE y NULL explícito.
Valida columnas no geométricas y tipos: enteros/números JSON finitos, bool estricto, texto/fechas ISO.
No convierte texto a número ni 0/1 a bool; el adaptador legacy conserva su casteo previo.
Tipos no soportados se rechazan; NULL conserva la lógica SQL de tres valores incluso con NOT.
Identificadores usan Identifier y valores placeholders; comparte casteo con WFS mediante values.py.
Los helpers existentes conservan firmas; no recibe Layer ni interpreta HTTP.
SpatialPredicate (#116) usa la geometría registrada de TableInfo y acepta within/intersects.
Combina `geom && polígono` con ST_Within/ST_Intersects exacto. Transforma el polígono al SRID
nativo en PostGIS; la columna queda desnuda. Los parámetros incluyen GeoJSON y SRID ligados.
El SRID de entrada se valida con el registro core (CRS desconocido → UnsupportedCRS 400);
el SRID nativo proviene de introspección, también para capas UTM. La salida se reproyecta aparte.
Se conservan huecos y bordes: WITHIN excluye puntos del borde, INTERSECTS los incluye.
EXPLAIN con seqscan deshabilitado verifica elegibilidad GIST, no una garantía de plan o latencia.

## Conversión compartida de atributos (#159)

`postgis.values.cast_attribute_value(value, data_type)` conserva el casteo legacy de WFS:
enteros con ancho PostgreSQL, flotantes/decimales finitos, booleanos, fechas ISO y texto.
Devuelve el escalar tipado o lanza ValueError/InvalidOperation/OverflowError; el adaptador
traduce esos fallos a InvalidFilter. No valida columnas ni construye SQL.

## Compatibilidad del adaptador WFS (#118)

compile_predicate añade el keyword legacy_values=False. Solo el adaptador legacy de WFS lo
activa tras revalidar filtros: conserva casteo, fechas, Decimal exacto y tipos heredados.
El modo normal sigue rechazando coerción de texto a número/bool y tipos no soportados.
Ningún modo cambia whitelist, operadores permitidos, Identifier ni placeholders.
