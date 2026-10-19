# integration

> Módulo: `src/atlas/integration.py` (+ `src/atlas/__init__.py`)

## Propósito

La cara pública de la librería: la clase `Atlas`, el `router` que se monta en la app FastAPI del
usuario y el ciclo de vida del pool. Es lo único que un usuario nuevo necesita entender (§6).

> El §33 del requerimiento proponía `fastapi.py`. Se llama `integration.py` para no colisionar con
> el paquete `fastapi` en imports absolutos.

## Contrato público

```python
# atlas/__init__.py
from atlas.integration import Atlas
from atlas.core.config import AtlasConfig
from atlas.layers.models import Layer

# atlas/integration.py
class AtlasContext(BaseModel):     # lo que reciben todos los routers
    config: AtlasConfig
    db: Database
    registry: LayerRegistry
    renderer: MapRenderer
    permission_checker: PermissionChecker | None
    log_hook: LogHook | None
    principal_dependency: Callable | None

class Atlas:
    def __init__(
        self, database_url: str, *, prefix: str = "/atlas",
        config: AtlasConfig | None = None,
        renderer: MapRenderer | None = None,          # default: PillowRenderer()
        permission_checker: PermissionChecker | None = None,
        principal_dependency: Callable | None = None, # Depends(...) de la app host
        log_hook: LogHook | None = None,
    ) -> None

    async def register_layer(self, source: str, **overrides) -> Layer
    def register(self, layer: Layer) -> Layer         # capa ya construida, sin I/O
    @property
    def router(self) -> APIRouter                     # /layers, /wms, /wfs, /maps
    @property
    def lifespan(self) -> AsyncContextManager         # abre y cierra el pool
    async def startup(self) -> None
    async def shutdown(self) -> None
```

Uso mínimo:

```python
app = FastAPI()
geo = Atlas(database_url="postgresql://...")

@app.on_event("startup")
async def _startup() -> None:
    await geo.startup()
    await geo.register_layer("geo.municipios", title="Municipios de Jalisco")

app.include_router(geo.router)
```

## Invariantes

- **Atlas no arranca ningún proceso ni servidor.** Vive dentro de la app del usuario (§6).
- El router se construye una sola vez, al primer acceso a `.router`. Registrar una capa después no
  cambia rutas: las rutas son genéricas y consultan el registry en cada petición.
- Las rutas de Atlas usan `AtlasRoute`, una subclase de `APIRoute` que captura `AtlasError` y
  devuelve `error_response`. `APIRouter` no admite handlers de excepción propios; no se modifica
  ningún handler ni configuración global de la app host.
- `AtlasContext` vive en `atlas.context` y se expone como `geo.context`. El `database_url` debe
  coincidir con el de una configuración explícita; una discrepancia lanza `ValueError`.
- `lifespan` puede pasarse a `FastAPI(lifespan=geo.lifespan)` y cierra el pool incluso si falla la
  app. El renderer por defecto es `PillowRenderer()`; se admite uno externo que cumpla `MapRenderer`.
  El hook recibe `OperationLog` de WFS y WMS, incluida duración, capas y cantidad de elementos.
  `/wms` despacha GetCapabilities, GetMap y GetFeatureInfo de WMS 1.3.0; los errores siguen
  el formato JSON común de Atlas.
- Los sub-routers reciben el `AtlasContext`; ningún módulo importa una instancia global de `Atlas`.
  Sin singletons.
- `register_layer` es async porque hace introspección. `register` es sync para capas ya construidas
  (útil para tests y para la persistencia de v0.2).
- El prefijo es configurable y por defecto `/atlas`: montar en la raíz colisionaría con las rutas
  de la app host.

## Dependencias

Importa: todos los paquetes.
No lo importa nadie (es la raíz).

## Fuera de alcance

- Un `main.py` ejecutable o un CLI de servidor. Atlas es librería (§3.2.11).
- Middlewares, CORS, auth: son de la app host.
- Configuración por archivo YAML/TOML. Se configura en Python.

## Referencias

`requirements.md` §3.1 (objetivo general), §6 (integración FastAPI), §33 (organización del paquete),
§42 (criterios de éxito).
