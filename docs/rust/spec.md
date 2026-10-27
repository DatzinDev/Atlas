# SDD — Atlas Rust

Contrato objetivo aprobado, no descripción del runtime actual. Épica [#203](https://github.com/DatzinDev/Atlas/issues/203).
Los IDs siguientes deben aparecer en issues y pruebas de aceptación. La API nueva es `/api/v1`.

## Objetivo y usuarios

Motor open source ligero para desarrolladores de geoportales, aplicaciones y publicación de datos.
El administrador registra datasets; el consumidor los descubre, filtra, visualiza y descarga.
Rust coordina la API, políticas y recursos; PostGIS almacena, filtra, reproyecta y codifica MVT.

## Requisitos del MVP

| ID | Contrato y aceptación |
|---|---|
| R01 Publicación | CRUD administrativo persistente de publicaciones; reiniciar conserva el catálogo; borrar nunca elimina la relación fuente. |
| R02 Introspección | Tablas/vistas de esquemas permitidos; geometría registrada explícita o inferida sin ambigüedad; atributos/CRS descubiertos y validados. |
| R03 Descubrimiento | Catálogo acotado y detalle solo autorizados; no publicar credenciales, schema/table ni políticas internas. |
| R04 Selección | Un modelo compartido por items/query, export y tiles; whitelist de propiedades y tipos derivados de introspección. |
| R05 Predicados | eq/ne/gt/gte/lt/lte/like/ilike/in/is_null/is_not_null y and/or/not; preservar lógica SQL de NULL. |
| R06 Espacio | Bbox y Polygon/MultiPolygon intersects/within; XY finitos, anillos cerrados y geometría válida; sin reparación silenciosa. |
| R07 Restricciones | AND(filtro fijo de publicación, filtro del usuario, bbox); ningún OR/NOT ni selección de campos puede ampliar lo publicado. |
| R08 Páginas | limit/offset y orden determinista con clave única; no contar todo automáticamente; respuesta con numberReturned y hasMore. |
| R09 GeoJSON | FeatureCollection en EPSG:4326 longitud/latitud; clave primaria conservada; selección de propiedades sin perder el identificador. |
| R10 MVT | XYZ 3857, ST_AsMVTGeom/ST_AsMVT, TileJSON; vacío válido 200; límite de candidatos/bytes produce 413 antes de enviar payload. |
| R11 Exportación | GeoJSON completo por cursor en lotes; sin paginación implícita; límites comprobados y limpieza al desconectar. |
| R12 Identidad | JWT externos RS256: firma, issuer, audience, exp obligatorios; validar nbf si existe; nunca confiar en claims sin verificar. |
| R13 Políticas | roles por dataset y operación view/query/download; atlas-admin administra pero no concede acceso implícito a los datos. |
| R14 Privacidad | Nueva publicación privada y denegada por defecto; acceso anónimo solo explícito; token inválido devuelve 401 incluso en dataset público. |
| R15 Recursos | Límites de parser, pool, consultas activas, tiempo, filas y bytes; saturación 503, presupuestos 413; nunca éxito parcial oculto. |
| R16 Operación | Binario/contenedor, health/readiness, logs estructurados sin secretos; shutdown drena trabajo y cierra conexiones. |
| R17 Compatibilidad | Python y consumidores existentes no cambian por publicar Rust; migración explícita y retorno probado antes de retirar versiones. |

## Consulta y tipos

Query es un objeto cerrado: filter, bbox, properties, sort, limit, offset. GET representa filter
como JSON UTF-8 codificado en URL; POST recibe el mismo modelo JSON. Duplicados y campos
desconocidos son 400. Bbox y geometría de selección son EPSG:4326; el servidor transforma la
selección al SRID nativo y mantiene indexable la columna. La validez topológica se comprueba
en PostGIS con tiempo acotado, sin añadir un motor geométrico Rust al MVP.

Los valores se validan contra el tipo PostgreSQL antes de ejecutar la selección: enteros con
rango, booleanos estrictos, números finitos, texto y fechas ISO compatibles. No hay coerción
libre ni expresiones SQL. Bigint y numeric se conservan sin convertirlos a f64; documentar
su serialización JSON como en Python, sin prometer precisión en clientes JavaScript.
Tipos no soportados no se exponen silenciosamente: se rechazan al publicar o se excluyen explícitamente.

MVP: tablas con PK simple y vistas con id_column declarado único/no nulo. Registrar verifica
esa condición; el publicador debe mantenerla tras cambios de datos. Consultas paginan con el
id como desempate. Offset no es un snapshot entre solicitudes; exportar usa una sola transacción.
Cambios incompatibles del esquema hacen fallar la consulta de forma controlada; PATCH vuelve
a inspeccionar la publicación. No hay descubrimiento ni refresco automático de todas las tablas.

## Acceso y publicación

Fuente elegida solo por administradores sobre una BD configurada y allowed_schemas; nunca por
el consumidor. Un dataset registra id, título/descripción, schema/table, geometría/id, propiedades
publicadas, filtro fijo, enabled y política. La introspección valida cada campo publicado.
public concede view/query anónimos; descarga anónima requiere public_download. Para privados,
cada operación exige intersección de roles; query/download requieren también view.
Un dataset invisible o deshabilitado devuelve 404 en rutas de datos; falta de una operación
sobre un dataset visible devuelve 403. Un rol atlas-admin gestiona el catálogo administrativo.
No se aceptan claves, issuer o rutas de JWKS suministradas por el token.

## Límites iniciales configurables

| Recurso | Default |
|---|---:|
| Página por defecto / máxima; offset máximo | 1.000 / 10.000; 100.000 |
| Catálogo por defecto / máximo | 100 / 1.000 |
| Filtro: bytes / profundidad / nodos | 65.536 / 8 / 100 |
| IN / pares XY | 1.000 / 10.000 |
| Pool mínimo / máximo; adquisición | 1 / 10; 5 s |
| Consulta SQL; stream total | 30 s; 300 s |
| Operaciones de datos simultáneas | 8; sin cola propia, rechazo 503 |
| MVT candidatos / bytes / zoom máximo | 100.000 / 8 MiB / 22 |
| MVT extent / buffer | 4.096 / 64 |
| Exportación features / bytes emitidos | 1.000.000 / 256 MiB |
| Body Query / administración | 128 KiB / 256 KiB |

Todas las consultas de datos usan transacciones de solo lectura; la cuenta de servicio solo
puede leer las fuentes y modificar el catálogo. Migraciones requieren comando explícito y rol
DDL separado. El semáforo cubre la vida real de la operación, incluidos streams y cancelación.
Export comprueba el conteo dentro de la misma transacción REPEATABLE READ antes de headers;
si el límite de bytes/tiempo se alcanza durante el stream, aborta y registra, nunca cierra JSON
como si estuviera completo. Los límites no prometen RSS exacta independiente de PostgreSQL.

## Exclusiones y aceptación global

Sin OGC, login, gestión de usuarios, UI, raster, imágenes, SQL arbitrario, joins/aggregaciones
dinámicas, escritura de features, múltiples bases, Redis, colas, plugins runtime ni SDK Python.
Vistas PostGIS publicadas resuelven lógica compleja predefinida.
El MVP termina con tabla y vista privadas/públicas administradas, reinicio persistente,
consulta/tiles/export coherentes y límites/cancelaciones probados contra PostGIS real.
No hay objetivo numérico de superioridad aprobado: publicar medición comparable y limitaciones.
