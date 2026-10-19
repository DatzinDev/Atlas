# Atlas — guía para agentes

Librería Python/FastAPI que publica capas PostGIS vía WMS y WFS. Import `atlas`, distribución
`atlas-geo`, layout `src/atlas/`.

## Antes de tocar código

1. Lee el issue completo: trae contrato, archivos y criterios de aceptación.
2. Lee `docs/components/<componente>.md` del componente que vas a tocar. Es corto y es la fuente
   de verdad del contrato público y sus invariantes.
3. Si tocas SQL, filtros o permisos, lee además `docs/components/security.md`.
4. Cada paquete en `src/atlas/` tiene su propio `CLAUDE.md` con las reglas locales.

No leas `docs/requirements.md` entero salvo que el issue lo pida: es el documento fuente, largo,
y los docs de componente ya destilan lo que aplica.

## Stack

Python 3.11+ · FastAPI · Pydantic v2 · psycopg3 async (`psycopg[binary,pool]`) · shapely · Pillow
· pytest + pytest-asyncio · ruff · mypy.

**No añadas dependencias.** Si crees que hace falta una, ábrelo como discusión en el issue y
escribe un ADR en `docs/adr/`; no la instales en el PR.

## Invariantes globales

Ningún PR puede romper estas. Están desarrolladas en `docs/conventions.md` y `security.md`.

1. **Nada de SQL por concatenación.** Identificadores con `psycopg.sql.Identifier`, valores con
   `%s`. Los nombres de columna se validan antes contra la whitelist de la capa.
2. **Toda reproyección es `ST_Transform` en PostGIS.** Nunca aritmética de coordenadas en Python.
3. **Todo listado está acotado** por `default_limit` / `max_limit` de `AtlasConfig`. Nada devuelve
   filas sin límite; las descargas van en streaming.
4. **Atlas no autentica.** Recibe un `principal` y consulta el `PermissionChecker` de la app host.
   Nada de usuarios, sesiones ni middleware propio.
5. **Se respetan las direcciones de dependencia** de `docs/architecture.md`. `rendering/` nunca
   importa `postgis/`; `core/` no importa nada del proyecto.
6. **Un módulo, un archivo de test.** `src/atlas/wfs/filters.py` → `tests/wfs/test_filters.py`.

## Comandos

```bash
docker compose up -d          # PostGIS de desarrollo con datos de prueba
pytest                        # tests (necesita el contenedor arriba)
pytest tests/wfs -x           # tests de un componente
ruff check . && ruff format .
mypy src/
```

## Alcance

Antes de añadir cualquier capacidad, aplica el principio rector (§45 de `requirements.md`):

> ¿Esta capacidad es necesaria para publicar, visualizar, consultar o descargar información de
> PostGIS desde una aplicación web?

Si no, no pertenece al núcleo. `docs/roadmap.md` lista lo que está explícitamente fuera de alcance.
**Implementa lo que dice el issue y nada más**: un PR que añade "de paso" algo que nadie pidió es
un PR que se devuelve.

## Git

- **`develop` es la rama de integración**: todo PR de un issue va contra `develop`, nunca contra
  `main`. `main` solo recibe releases desde `develop`.
- Rama por issue: `type/nombre-corto` (`feat/wfs-get-feature`), cortada desde `develop`.
- `main` y `develop` son ramas protegidas por convención: nunca commit directo a ninguna.
- Conventional Commits de **una sola línea**, sin cuerpo y sin trailers de co-autoría:
  `feat(wfs): add GetFeature endpoint with GeoJSON output`
- Si el commit necesita un párrafo para justificarse, pártelo en varios commits.
- Un PR cierra un issue: `Closes #N`.

## Idioma

Docs, issues y PRs en **español**. Código, docstrings, mensajes de error, commits y labels en
**inglés**.
