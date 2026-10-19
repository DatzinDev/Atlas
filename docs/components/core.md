# core

> Módulo: `src/atlas/core/`

## Propósito

Piezas transversales que todo el resto usa: configuración, jerarquía de excepciones, tipos
compartidos, registro de CRS y enganche de logging. No contiene lógica de negocio ni SQL.

## Contrato público

### `core/config.py`

Fuente única de todos los límites y opciones de conexión.

```python
class AtlasConfig(BaseModel):
    # frozen y extra="forbid": inmutable y sin campos desconocidos
    database_url: str                # no vacío; se recortan espacios
    default_limit: int = 1000        # features devueltos si el cliente no pide límite
    max_limit: int = 10000           # techo absoluto para consultas paginadas
    max_download_features: int | None = None   # None = sin límite en descargas por streaming
    default_srid: int = 4326
    pool_min_size: int = 1
    pool_max_size: int = 10
    query_timeout: float = 30.0      # segundos
    max_image_size: int = 4096       # lado máximo en píxeles de una imagen renderizada

    def resolve_limit(self, requested: int | None) -> int
```

### `core/exceptions.py`

Jerarquía plana: un solo nivel bajo `AtlasError`, para que un `except AtlasError` en la app host
cubra toda la librería.

```python
class AtlasError(Exception):
    status_code: int = 500
    code: str = "atlas_error"

    def __init__(self, message: str, **details: Any) -> None: ...

    message: str                # texto para quien lee la respuesta
    details: dict[str, Any]     # contexto estructurado y público; {} si no hay


def error_response(exc: AtlasError) -> dict[str, Any]:
    """-> {"error": {"code": str, "message": str, "details": dict}}"""
```

Cada subclase solo redefine `status_code` y `code`:

| Excepción | `status_code` | `code` | Cuándo |
| --- | --- | --- | --- |
| `LayerNotFound` | 404 | `layer_not_found` | No hay capa registrada con ese nombre |
| `ServiceDisabled` | 404 | `service_disabled` | La capa existe pero no publica ese servicio |
| `UnsupportedCRS` | 400 | `unsupported_crs` | El CRS pedido no está en `SUPPORTED_CRS` |
| `UnsupportedFormat` | 400 | `unsupported_format` | El servicio no produce ese formato |
| `InvalidParameter` | 400 | `invalid_parameter` | Parámetro ausente o malformado (OGC `Missing`/`InvalidParameterValue`) |
| `InvalidFilter` | 400 | `invalid_filter` | El filtro no parsea o nombra columnas fuera de la whitelist |
| `InvalidGeometry` | 400 | `invalid_geometry` | Geometría malformada o inservible para la operación |
| `QueryTooLarge` | 413 | `query_too_large` | Servirla excedería un límite de `AtlasConfig` |
| `QueryTimeout` | 504 | `query_timeout` | Se superó `AtlasConfig.query_timeout` |
| `DatabaseError` | 500 | `database_error` | PostGIS no pudo servir la petición |
| `PermissionDenied` | 403 | `permission_denied` | El `PermissionChecker` de la app host lo rechazó |

`DatabaseError` es la única con estado propio, porque el mensaje del driver no puede salir a la
respuesta:

```python
class DatabaseError(AtlasError):
    PUBLIC_MESSAGE: str         # lo que se le dice al cliente, diga lo que diga el driver
    technical_message: str      # el mensaje real de psycopg, solo para el log
```

### `core/crs.py`

```python
SUPPORTED_CRS: dict[int, CRSInfo]       # 4326 y 3857 en v0.1

def parse_crs(value: str) -> int: ...   # "EPSG:3857" | "urn:ogc:def:crs:EPSG::3857" | "3857" -> 3857
def require_crs(srid: int) -> CRSInfo: ...  # lanza UnsupportedCRS
```

### `core/types.py`

```python
BBox = tuple[float, float, float, float]        # minx, miny, maxx, maxy
GeometryType = Literal["Point", "LineString", "Polygon", "MultiPoint", ...]
```

### `core/logging.py`

```python
class OperationLog(BaseModel):
    layer: str | None
    operation: str
    execution_time: float
    feature_count: int | None
    user: str | None
    error: str | None


LogHook = Callable[[OperationLog], None]
```

## Invariantes

**Del paquete**

- `core` no importa nada de otro paquete de Atlas. Es la hoja del grafo de dependencias.

**Configuración**

- Ningún límite se aplica fuera de `AtlasConfig`. Nada de constantes mágicas dispersas.
- `resolve_limit` **acota, no rechaza**: un límite pedido por encima de `max_limit` devuelve
  `max_limit`. Solo un límite no positivo es error (`ValueError`). Una config con
  `default_limit > max_limit` no se puede construir.
- Una config incoherente no se puede construir: ni `default_limit > max_limit` ni
  `pool_min_size > pool_max_size`. El error es de validación, no de runtime.
- `AtlasConfig` es **inmutable** (`frozen`) y **rechaza campos desconocidos** (`extra="forbid"`).
  Un `max_limmit=50` mal escrito es un error de validación, no un límite ignorado en silencio;
  y ningún componente puede saltarse los validadores mutando la config compartida.

**Excepciones**

- Toda excepción que pueda llegar al usuario hereda de `AtlasError` y tiene `status_code` y `code`.
- El `code` es parte del contrato público: no se renombra sin bump de versión.
- Ningún `code` se repite: es lo que el cliente usa para distinguir un fallo de otro.
- `error_response` devuelve siempre la misma forma, con `details` como dict —vacío si no hay
  contexto—, para que el cliente nunca tenga que comprobar si la clave existe.
- `details` es payload **público**. Nada que el cliente no deba ver entra ahí: el mensaje de psycopg
  vive en `DatabaseError.technical_message`, fuera de la respuesta, porque nombra esquemas, tablas y
  a veces valores literales del query.

**CRS**

- Los CRS soportados viven en un único diccionario. Añadir uno es añadir una entrada, no un `if`.
- `require_crs` también admite texto: `require_crs("CRS:84")` conserva el SRID 4326 con ejes `xy`,
  sin mutar el registro de EPSG:4326 (`yx`). `parse_crs` devuelve solo el SRID y no retiene el alias.

**Logging**

- El logging es un hook opcional que la app host provee. Atlas nunca configura `logging.basicConfig`
  ni escribe a stdout por su cuenta.

## Dependencias

Importa: nada del proyecto. Solo `pydantic` y stdlib.
Lo importan: todos.

## Fuera de alcance

- Motor de reproyección propio — las transformaciones son `ST_Transform` en `postgis/`.
- Autenticación, sesiones o usuarios (§17).
- Caché (v0.3).

## Referencias

`requirements.md` §20 (CRS), §23 (límites), §25 (logging), §26 (errores).
