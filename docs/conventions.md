# Convenciones

## Idioma

- **Español**: documentación, issues, PRs, plantillas.
- **Inglés**: nombres de módulos, clases, funciones, variables, docstrings, mensajes de error,
  commits y labels.

## Código

- Python 3.11+. Todo el código público lleva anotaciones de tipo; `mypy --strict` sobre `src/`.
- Async por defecto en cualquier función que toque la base de datos o devuelva una respuesta HTTP.
- Modelos de datos con Pydantic v2. Nada de dataclasses para lo que cruza la frontera HTTP.
- Un módulo hace una cosa. Si un archivo pasa de ~250 líneas, probablemente son dos módulos.
- Sin abstracciones especulativas: una interfaz solo existe si hay dos implementaciones reales o
  el requerimiento la pide explícitamente (`MapRenderer`, §10).

## SQL

Regla dura, sin excepciones (§13.1 de `requirements.md`):

```python
# MAL — inyección garantizada
cur.execute(f"SELECT {cols} FROM {table} WHERE {col} > {value}")

# BIEN — identificadores compuestos, valores parametrizados
from psycopg import sql
cur.execute(
    sql.SQL("SELECT {cols} FROM {table} WHERE {col} > %s").format(
        cols=sql.SQL(", ").join(map(sql.Identifier, columns)),
        table=sql.Identifier(schema, table),
        col=sql.Identifier(column),
    ),
    (value,),
)
```

- Los nombres de columna vienen **siempre** de la whitelist derivada de la introspección de la
  capa. Nunca directamente del query string.
- Los operadores vienen de un `dict` fijo de operadores permitidos (§13.2).
- Toda transformación de CRS se hace con `ST_Transform` en PostGIS, nunca en Python.
- Los filtros espaciales usan `&&` o `ST_Intersects` sobre la columna de geometría sin envolverla
  en funciones, para no invalidar el índice GIST (§22).

## Errores

Toda excepción hereda de `atlas.core.exceptions.AtlasError` y define `status_code`. El handler
registrado por `Atlas` las convierte en respuestas consistentes. Nunca se deja escapar un
`psycopg.Error` crudo hacia el usuario.

## Tests

- `pytest` + `pytest-asyncio`. Los tests de PostGIS usan el contenedor de `compose.yaml`.
- **Un archivo de test por módulo**: `src/atlas/wfs/filters.py` → `tests/wfs/test_filters.py`.
- Cada issue entrega su test. Un PR sin test es un PR incompleto, salvo que el issue diga
  explícitamente lo contrario.
- Los tests de seguridad de filtros y SQL no son opcionales: cada operador y cada whitelist
  necesita un caso que verifique que lo no permitido se rechaza.

## Commits y ramas

- Conventional Commits de **una sola línea**: `type(scope): descripción simple`.
- Sin cuerpo. Si el commit necesita un párrafo para justificarse, hay que partirlo.
- Sin trailers de co-autoría.
- Una rama por issue: `type/nombre-corto` (ej. `feat/wfs-get-feature`), cortada desde `develop`.
- Los PRs de issues van contra `develop`. `main` solo recibe releases.
- Nunca commit directo a `develop` ni a `main`.

## Documentación

- Cambiar el contrato público de un componente obliga a actualizar su `docs/components/*.md` en
  el mismo PR.
- Una decisión que descarta una alternativa razonable (dependencia, motor, protocolo) va a
  `docs/adr/` antes de implementarse.
