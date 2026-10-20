# ADR 0005 — Predicados compartidos de consulta

**Estado:** propuesto para revisión; implementación pendiente (#105, #109–#133).
**Fecha:** 2026-10-19

## Decisión y responsabilidades

| Capa | Responsabilidad futura | No conoce |
|---|---|---|
| core | Predicate validado y límites AtlasConfig | SQL, Layer, HTTP |
| postgis | compile_predicate(info: TableInfo, predicate: Predicate) → (sql.Composed, list) | Layer, HTTP |
| wfs/wms/tiles/exports | Parsear transporte, autorizar, combinar predicados | SQL fuera de postgis |
| aplicación | Principal, permisos, filtro fijo de publicación y reglas de producto | — |

Ni WMS ni tiles importan filtros de WFS. Los helpers SQL existentes conservan sus firmas;
el compilador nuevo es aditivo. `parse_filters`/`compile_filters` de WFS se mantienen como
adaptadores compatibles, incluidos operadores y parámetros actuales. No cambia su API Python
sin un issue explícito de migración. ADR 0003 conserva el alcance OGC; esta extensión es propia.

## Entrada única

`ATLAS_FILTER` contiene JSON para una capa (WFS, descargas y MVT).
WMS usa solo `ATLAS_FILTERS`, objeto nombre de capa → predicado; ausencia equivale a true.
Una clave fuera de LAYERS es InvalidFilter. Ambos parámetros juntos se rechazan.
No se reinterpretan `FILTER` XML ni `CQL_FILTER` como JSON; una semántica no soportada
se rechaza explícitamente, nunca se ignora ni se anuncia como Filter Encoding conforme.

```json
{"op":"and","args":[{"op":"gte","field":"poblacion","value":100000},{"op":"is_null","field":"baja"}]}
```

```json
{"municipios":{"op":"intersects","geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]},"srid":4326}}
```

La segunda entrada ejemplifica ATLAS_FILTERS. El modelo recursivo solo admite claves conocidas:
- `and`/`or`: args no vacío; `not`: arg único. Sin mezclas field/args/arg.
- `eq, ne, gt, gte, lt, lte, like, ilike`: field y value escalar no nulo.
- `in`: field y value lista no vacía de escalares no nulos compatibles.
- `is_null`/`is_not_null`: solo field. No hay comparación ordinaria con null.
- `within`/`intersects`: geometry Polygon/MultiPolygon GeoJSON 2D y srid explícito.
  Se opera sobre la geometría registrada; nunca se acepta nombre de columna geométrica del cliente.

Comparaciones combinadas siguen lógica SQL de tres valores: solo TRUE selecciona filas;
NOT sobre NULL no equivale a is_not_null. LIKE/ILIKE usan patrones SQL parametrizados.
Tipos se comprueban con TableInfo: números finitos, booleanos estrictos, textos y fechas ISO
para columnas compatibles; no se aceptan expresiones, coerción libre ni campos desconocidos.
Geometrías no vacías, finitas y topológicamente válidas; no se reparan silenciosamente.
BBox y polígono se transforman al SRID nativo; && prefiltra y ST_Within/ST_Intersects
confirman exactamente, dejando la columna indexable. No se calculan predicados en Python.

## Límites y errores

AtlasConfig incorporará defaults positivos: filter_max_bytes=65536 (UTF-8),
filter_max_depth=8 (raíz=1), filter_max_nodes=100, filter_max_in_values=1000 y
filter_max_coordinates=10000 (pares XY acumulados). Validar bytes antes de JSON y
profundidad/nodos antes de recursión/SQL; listas y geometrías también consumen esos límites.
JSON inválido, claves duplicadas/desconocidas, operadores/tipos/CRS inválidos y exceso
producen InvalidFilter (400), antes de adquirir conexión; no se expone SQL ni valores sensibles.
Se conserva statement_timeout; los presupuestos de ejecución son independientes del parser.

## Composición y migración

Siempre `AND(filtro_fijo, filtro_legacy, filtro_usuario, bbox)`; OR/NOT del cliente no pueden
eliminar restricciones del host. La autorización se hace antes de consultar, sin cambiar
los permisos view/query/download existentes. Un filtro fijo inválido falla cerrado al registrar.
Conteo, página, imagen, info y descarga reutilizan el mismo predicado, no el mismo límite:
WFS pagina con orden estable; hits cuenta sin geometría; descargas recorren el conjunto con
streaming. WMS/MVT tienen presupuestos propios y error explícito al excederlos, sin entregar
una página como si fuese todo el mapa. No se elevan max_limit/default_limit globalmente.

Si se cachea contenido, la identidad incluye predicado normalizado completo (también el fijo),
propiedades/orden/CRS/estilo y revisión de publicación/datos; cambios invalidan entradas.
No compartir contenido privado entre principales; sin identidad segura se evita el caché.
No se añade un caché nuevo en este ADR.

## Consecuencias y verificación

Un modelo pequeño evita parsers divergentes; mantenerlo acotado cuesta validación explícita.
No se incorpora un parser CQL/SLD/GML ni un árbol SQL aportado por el cliente.
Adaptadores GeoServer, grupos, calendarios, geometrías IIEG/INEGI y autenticación quedan
fuera de Atlas. La implementación probará rechazos, NULL/NOT, inyección, límites y paridad
entre conteo/página/descarga/GetMap/GetFeatureInfo sobre fixtures, sin afirmar que ya existe.
