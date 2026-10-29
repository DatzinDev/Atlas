# Verificación de filtros y selección espacial (#220–#222)

## Contrato implementado

Parser cerrado con límites antes de descender, escalares sin coerción, precisión
bigint/numeric preservada, Polygon/MultiPolygon XY y claves duplicadas rechazadas.
SQLx enlaza valores; identificadores provienen de atributos publicados e introspección.
La entrada común `selection::prepare` combina siempre filtro fijo AND usuario AND bbox.
Su resultado se añade al WHERE mediante `Selection::append`, en la misma transacción
de solo lectura y después de autorizar. No hay nuevos endpoints HTTP en este bloque.

La topología se comprueba en PostGIS antes de seleccionar filas. Geometrías inválidas,
vacías, XY fuera del dominio 4326 o transformaciones inválidas fallan sin reparación.
El bbox debe producir una selección poligonal válida; no se interpreta un bbox invertido
como cruce del antimeridiano. La columna fuente permanece en su CRS nativo y sin
envolver en ST_Transform: && prefiltra y ST_Intersects/ST_Within confirma. Reproyectar
la salida pertenece al consumidor posterior del WHERE.

## Reproducción

Desde `rust/`, con PostGIS descartable accesible en `ATLAS_TEST_DATABASE_URL`:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

La suite falla si no se proporciona PostGIS real. Usar el procedimiento de
[fundamentos](foundation-verification.md); no apuntar a fuentes o roles de producción.
No se necesitan dependencias nuevas ni modificar el runtime Python.

## Casos cubiertos

- Parser: límites exactos/excedidos de bytes, profundidad, nodos, IN y pares XY;
  anidamiento ofensivo; tipos, operadores, claves duplicadas/desconocidas y geometrías.
- Regresión: un objeto con `$serde_json::private::Number` nunca se acepta como escalar,
  ni en comparación ni en IN, aunque Serde tenga arbitrary_precision activado.
- Atributos: bigint máximo y numeric de 23 dígitos, NULL/NOT, fechas, booleanos,
  LIKE/ILIKE, nombres con comillas, columnas ocultas y entradas ofensivas rechazadas.
- Espacio: topología inválida, huecos, borde incluido en intersects y excluido en within,
  reproyección 4326 a 3857, CRS desconocido y selección fuera del dominio geográfico.
- Composición: OR/NOT mezclado con espacio nunca amplía el filtro fijo; bbox se suma
  como AND. Se prueban IDs efectivos contra PostGIS, no solo texto SQL.
- EXPLAIN con enable_seqscan deshabilitado confirma elegibilidad de GIST.

## Límites de la evidencia

EXPLAIN no garantiza el plan de producción ni demuestra mejoras de latencia.
No se han medido cargas nuevas ni comparado GeoServer. Paginación, rutas HTTP,
exports y tiles se integran en issues posteriores. No hay cambios de consumidores,
merge automático ni despliegue. El borrador local inicial se conserva fuera del stack.
