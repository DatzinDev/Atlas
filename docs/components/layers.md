# layers

> Módulo: `src/atlas/layers/`

## Propósito

El componente central (§7). Define qué es una capa publicable, mantiene el catálogo en memoria y
resuelve permisos por capa y operación.

## Contrato público

```python
# layers/models.py
class LayerServices(BaseModel):
    wms: bool = True; wfs: bool = True; wmts: bool = False; wcs: bool = False

class Layer(BaseModel):
    name: str                       # identificador único, url-safe
    title: str                      # por defecto: name capitalizado
    description: str | None = None
    schema_name: str; table_name: str
    geometry_column: str; geometry_type: GeometryType; srid: int
    id_column: str | None = None
    is_view: bool = False           # conserva el tipo de relación al reconstruir TableInfo
    attributes: list[ColumnInfo]
    extent: BBox | None = None      # en EPSG:4326
    enabled: bool = True
    searchable: bool = True
    downloadable: bool = True
    public: bool = True
    services: LayerServices = LayerServices()
    style: None = None              # Style | None se habilita con el issue #28
    permissions: list[str] = []     # ver permissions.py
    min_zoom: int | None = None; max_zoom: int | None = None

    @property
    def table_info(self) -> TableInfo    # lo que consume postgis/
    @property
    def attribute_names(self) -> list[str]   # whitelist de columnas
    def supports(self, service: str) -> bool

# layers/registry.py
class LayerRegistry:
    def register(self, layer: Layer) -> Layer
    def get(self, name: str) -> Layer                  # lanza LayerNotFound
    def require_service(self, name: str, service: str) -> Layer  # lanza ServiceDisabled
    def list(self, *, enabled_only: bool = True) -> list[Layer]
    def remove(self, name: str) -> None
    def __contains__(self, name: str) -> bool

# layers/metadata.py
async def build_layer(
    db: Database, source: str, *, name: str | None = None, **overrides
) -> Layer
# source = "geo.municipios" | "municipios" (schema por defecto: public)
# Lo que no se pasa como override se descubre por introspección (§19).

# layers/permissions.py
Operation = Literal["view", "query", "download", "admin"]
PermissionChecker = Callable[[Layer, Operation, Any], bool | Awaitable[bool]]
def default_permission_name(layer: Layer, op: Operation) -> str   # "atlas.municipios.view"
async def check_permission(checker, layer, op, principal) -> None  # lanza PermissionDenied
```

## Invariantes

- `Layer.name` es único en el registry y es lo que aparece en URLs y en `GetCapabilities`.
- Una capa con `enabled=False` no aparece en ningún capabilities ni responde a ninguna operación.
- `attributes` **nunca** incluye la columna de geometría: es la whitelist que protege el SQL.
- El modelo elimina la geometría de `attributes` al construirlo. `attribute_names` y `table_info`
  también la excluyen si luego se modifica la lista. `table_info` conserva `is_view` e `id_column`.
- `name` acepta solo `[a-z0-9_-]`, sin cadena vacía. Si se omite `title`, usa `name.capitalize()`.
- `supports` requiere la capa habilitada y el servicio activado; un servicio desconocido devuelve
  `False`. WMS y WFS están activos por defecto, WMTS y WCS no.
- Hasta implementar los modelos del #28, `style` solo acepta `None`; no se admiten estilos sin
  validar ni se adelanta la implementación de rendering.
- El registry es un `dict` en memoria y no persiste nada. La persistencia llega en v0.2 y se
  construirá **encima** de esta interfaz, sin cambiarla.
- Atlas no decide si un usuario tiene un permiso: llama al `PermissionChecker` que provee la app
  host y respeta su respuesta (§17). Si no hay checker configurado, todo se permite.
- Las cuatro operaciones (`view`, `query`, `download`, `admin`) se comprueban por separado (§18).

## Dependencias

Importa: `core`, `postgis`, `styles` (solo el tipo `Style`).
Lo importan: `wfs`, `wms`, `exports`, `api`, `integration`.

## Fuera de alcance

- Autenticación y gestión de usuarios (§17): la app host las provee.
- Persistencia del catálogo (v0.2, §8).
- Generación de XML de capabilities — eso vive en `wms/` y `wfs/`.

## Referencias

`requirements.md` §7 (Layer Registry), §8 (persistencia futura), §17 (auth), §18 (autorización),
§19 (introspección).
