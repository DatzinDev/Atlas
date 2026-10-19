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
async def table_extent(db: Database, info: TableInfo, srid: int = 4326) -> BBox | None

# postgis/queries.py
def build_feature_query(
    info: TableInfo, *, columns: Sequence[str] | None = None,
    where: Sequence[sql.Composed] = (), params: Sequence = (),
    geometry_as: Literal["geojson", "wkb", "none"] = "geojson",
    target_srid: int | None = None, limit: int | None = None, offset: int = 0,
) -> tuple[sql.Composed, list]

def bbox_predicate(info: TableInfo, bbox: BBox, bbox_srid: int) -> tuple[sql.Composed, list]
def count_query(info: TableInfo, where, params) -> tuple[sql.Composed, list]

# postgis/geometry.py
def to_geojson_feature(row: dict, id_column: str | None) -> dict
```

## Invariantes

- **Ningún f-string, `%` ni `.format()` de Python sobre SQL.** Identificadores con
  `psycopg.sql.Identifier`, valores con placeholders `%s`.
- `build_feature_query` valida cada nombre de columna contra `info.columns` y lanza `InvalidFilter`
  si no está. No confía en su llamador.
- Los predicados espaciales dejan la columna de geometría desnuda a la izquierda
  (`geom && ST_MakeEnvelope(...)`), nunca `ST_Transform(geom, X) && ...`, para no perder el índice
  GIST (§22). Se transforma el bbox al SRID de la tabla, no la tabla al del bbox.
- La reproyección de salida (`ST_Transform` sobre el SELECT) es lo único que sí envuelve la
  geometría, y va después del filtrado.
- `stream()` usa cursor con nombre. Ninguna consulta de descarga materializa el dataset completo.
- `inspect_table` funciona igual para tablas y para vistas; si `geometry_columns` no resuelve, cae
  a inspeccionar el tipo real de la columna.
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
