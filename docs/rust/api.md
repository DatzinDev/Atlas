# Contrato REST del MVP

Contrato del MVP experimental [OpenAPI 3.0.3](openapi.yaml); evidencia en [aceptación](acceptance.md). Requisitos [R01–R17](spec.md).
Prefijo fijo de versión `/api/v1`; los clientes no usan rutas Python como aliases.

## Transporte y coherencia

Query es idéntico para GET items y POST query. filter en GET es JSON codificado en URL;
bbox y properties usan listas separadas por comas; sort usa `field:asc,other:desc`.
GET admin/listado solo admite limit/offset. Export acepta filter/bbox/properties;
tiles y TileJSON solo filter/properties porque XYZ define su ventana.
bbox es `minx,miny,maxx,maxy` en EPSG:4326 con `-180 ≤ minx < maxx ≤ 180` y
`-90 ≤ miny < maxy ≤ 90`; fuera de eso (incluido área cero) es 400 invalid_request.
Un bbox que cruza el antimeridiano (`minx > maxx`) se rechaza: el cliente parte la consulta
en dos. Todo error causado por el bbox es invalid_request; los de geometrías de filter son
invalid_filter. En datasets EPSG:3857 bbox y polígonos se recortan a latitud
±85.0511287798066 (no es error; un polígono sin parte dentro no selecciona filas). Si el CRS
nativo no es 4326 se densifican cada 1° antes de transformar; una selección transformada
inválida, vacía o sin área es 400, nunca un 200 casi vacío. Parámetros desconocidos o repetidos
y claves JSON duplicadas son 400, no se ignoran. No aceptar ATLAS_FILTER, CQL o SQL del cliente.
Todas las rutas usan un único parser de query string con reglas idénticas: los segmentos vacíos
(`&&`, `&` final) se ignoran; par sin `=`, clave no admitida por la ruta o repetida, `%` mal
formado o UTF-8 inválido tras decodificar `%`/`+` son 400 invalid_request; un valor vacío es 400
invalid_request (limit, offset, bbox, properties, sort, format) salvo `filter=`, que es 400
invalid_filter. `format` solo existe en export y solo admite `geojson`; las rutas sin parámetros
(detalle, POST query, admin salvo listado) rechazan cualquier clave.
POST query y POST admin exigen una sola cabecera Content-Type `application/json` (sin distinguir
mayúsculas; solo admite el parámetro `charset=utf-8`); PATCH admite además
`application/merge-patch+json`. Otro valor, ausente o repetido es 400 invalid_request; en admin
se evalúa tras la autorización (403 primero) y en query antes del cuerpo y del catálogo.
`null` en cualquier campo de Query o del POST de publicación es 400 invalid_request.
Valores de filtro por tipo de columna: numeric acepta cualquier número JSON y compara exacto;
float4/float8 e int2/int4/int8 fuera de rango son 400 invalid_filter. date solo `YYYY-MM-DD`
(fecha inexistente es 400); timestamp ISO sin `Z` ni offset; timestamptz con o sin offset,
sin offset en UTC (sesión fijada por SQLx). Un string solo lo acota `filter_bytes` (65.536 por
defecto); no hay límite propio por string.
POST query no crea recursos ni persiste consultas. GET/POST producen la misma selección.
Una página items/query cuyo cuerpo superaría `limits.page_bytes` es 413 query_too_large,
sin truncar ni cambiar numberReturned/hasMore.
Todas las respuestas de datos dinámicas tienen Cache-Control: no-store; no hay caché MVT inicial.
Los tiles de datasets públicos también van con no-store y sin compresión: es intencional en el MVP.
Si se quiere gzip o caché, se configuran en el proxy y solo para datasets públicos.
GeoJSON EPSG:4326; tiles XYZ/MVT EPSG:3857. TileJSON 3.0 incluye URL, source-layer,
minzoom/maxzoom, bounds y attribution disponibles. Las URLs respetan el prefijo del despliegue
configurado; no se confía en cabeceras forwarded arbitrarias. La plantilla de TileJSON repite
filter/properties como query (filter primero, codificado salvo no reservados de RFC 3986) y
fields se limita a las propiedades pedidas. Sin public_url la URL es una ruta absoluta
(/api/v1/...) y el binario lo avisa al arrancar con el evento public_url_missing; los clientes
que necesiten URL absoluta con host requieren public_url.
En MVT, salvo SRID nativo 3857, la geometría se recorta en EPSG:4326 (`ST_ClipByBox2D`) contra
la envolvente del tile expandida por el buffer y transformada en PostGIS antes de pasar a 3857;
lo que queda fuera de ±85.0511° no se codifica y un vértice en ±90 nunca hace fallar el tile.
Si el SRID nativo no es 3857 ni 4326, la envolvente expandida se densifica con
`ST_Segmentize(env, ancho/16)` antes de transformarla al SRID nativo para seleccionar candidatos.
Los tags MVT de date/timestamp/timestamptz usan el mismo texto ISO 8601 que GeoJSON; numeric
va como texto y NaN/±Infinity como NULL. El id MVT se decide por el tipo de id_column inspeccionado
(guardado en metadata como id_type): enteros en [0, 2^53−1] se codifican; texto, negativos o
mayores van sin id. Metadata almacenada sin id_type válido se trata como esquema cambiado.
La migración rellena id_type desde pg_catalog sin tocar revision ni ETag; las filas que no
resuelve quedan sin id_type y se leen como schema_changed.

El catálogo público describe propiedades publicadas; jamás devuelve source, filter fijo,
roles, SQL o credenciales. El administrativo sí devuelve configuración sin credenciales.
El listado público prefiltra en SQL las publicaciones habilitadas que son públicas o comparten
un view_role con el principal (ninguno si es anónimo); la validación y la política en Rust
siguen decidiendo, con la misma paginación y hasMore.
PATCH usa Merge Patch: campos omitidos se conservan; null solo borra campos opcionales;
la configuración resultante se valida e inspecciona como en POST. id no cambia con PATCH.
Publicar relación ambigua exige geometry_column explícita. id_column es PK simple o clave
de vista única/no nula verificada: en una vista materializada con índice único válido, sin
predicado ni expresiones y cuya única clave es id_column, solo se comprueba que no haya id NULL;
en otro caso se recorre la vista entera (sujeto a statement_timeout, exceso 504 query_timeout).
Source solo referencia la BD configurada.
Atributos e id se aceptan solo si su tipo es por OID un tipo base admitido de pg_catalog;
dominios, enums y homónimos de otros esquemas son 400 invalid_request. La geometría debe ser
`geometry` del esquema de la extensión PostGIS. data_type conserva el nombre canónico. POST y
PATCH verifican SELECT del rol actual sobre geometría, id y propiedades publicadas; si falta
alguno, 400 invalid_request sin indicar cuál.
El filter fijo (solo admin) puede usar cualquier columna inspeccionada aunque no esté en
properties: cada campo debe existir con tipo admitido (no geometría) y SELECT, o 400
invalid_request. Esas columnas se guardan en metadata.filter_attributes y ninguna salida
(detalle público/admin, TileJSON, GeoJSON, MVT) las expone. Filtros, properties y sort del
cliente solo ven propiedades publicadas: un campo oculto es 400 invalid_filter en filter e
invalid_request en properties/sort. Si la columna oculta desaparece, 409 schema_changed.
Tipo y SRID salen del typmod; si falta alguno, se infiere recorriendo todas las filas no nulas
(sujeto a statement_timeout, exceso 504 query_timeout). Se exige un único SRID existente en
spatial_ref_sys, solo 2D (Z/M es 400) y un único tipo o exactamente X+MultiX (POINT, LINESTRING,
POLYGON), publicado como MultiX; otra mezcla es 400. Una relación sin filas no nulas y sin typmod
es 400: declarar la columna como geometry(Tipo,SRID).
El detalle y el listado público (y metadata admin) incluyen extent [minx,miny,maxx,maxy] en
EPSG:4326 o null si no hay filas. Se calcula en POST/PATCH en la misma inspección: tablas y vistas
materializadas usan ST_EstimatedExtent (si no hay estadísticas, ST_Extent); vistas, ST_Extent.
Se reproyecta con ST_Transform en PostGIS y se acota a [-180,-90,180,90], sujeto a
statement_timeout (exceso 504 query_timeout). Es una foto del momento de publicar: no sigue los
cambios de datos y se refresca con PATCH. Un extent guardado no finito, invertido o fuera de rango
es 409 schema_changed; filas antiguas sin el campo devuelven null.
ETag es la revision entre comillas en detalle administrativo/POST/PATCH. If-Match es obligatorio
para PATCH/DELETE; ausente o malformado es 400, revisión distinta 412. DELETE devuelve 204.
No usar ETag de datos: la revisión del catálogo no identifica cambios externos en PostGIS.
POST con un id existente es siempre 409 publication_conflict, aunque la configuración sea
idéntica; un reintento hace GET y compara. DELETE y luego POST con el mismo id crea una
publicación nueva con revision 1: un ETag anterior no la identifica.

## Autorización y errores

Bearer JWT RS256 emitido externamente, sub/iss/aud/exp obligatorios y roles array de strings.
Sin token: Principal anónimo. Si hay token, siempre verificarlo antes de aplicar acceso público.
Solo atlas-admin usa /admin; no equivale a autorización de query/download.
Un anónimo en /admin, o en una descarga de un dataset público sin public_download, recibe 403
permission_denied sin `WWW-Authenticate`: es intencional, porque el acceso no depende solo de
autenticarse (también hace falta el rol).
Los roles se comparan de forma exacta y distinguiendo mayúsculas (`Admin` ≠ `admin`). Se leen
del claim `roles` en la raíz del token; el mapeo configurable del claim se trata en #232.
Catálogo filtra por view; detalle/items/tiles exigen view+query; export view+download.
view_roles, query_roles y download_roles admiten hasta 1.000 roles cada una; cada rol tiene
de 1 a 128 caracteres sin espacios (incluido U+FEFF) ni caracteres de control. Fuera de eso
es 400 invalid_request al publicar o con PATCH. Si se quita el rol de la publicación, el token
que aún lo traiga ya no autoriza desde la siguiente petición, porque no hay caché de catálogo;
un stream ya autorizado sigue hasta terminar. Si el emisor deja de otorgar el rol, el token
emitido lo conserva hasta exp; esa ventana la fija el emisor.

Una publicación guardada que ya no decodifica o no valida (roles fuera de esa regla, una lista
que no es un arreglo de strings, un esquema retirado de allowed_schemas) deja de leerse, pero no
bloquea el listado administrativo: ocupa su lugar como `{"id", "revision", "error":
"schema_changed"}`, sin config ni metadata, y la página sigue en 200 con el mismo orden y hasMore.
Su detalle administrativo responde 409 schema_changed con `ETag: "<revision>"`; PATCH sigue en 409
(lee antes de escribir) y DELETE con ese ETag en If-Match la retira (204). En las rutas públicas no
hay 409: la publicación desaparece del catálogo y su detalle es 404. Antes de desplegar, detectar
las de roles inválidos con un SELECT de solo lectura, que solo requiere SELECT sobre
atlas_catalog.datasets (no el rol DDL):

```sql
SELECT id, revision FROM atlas_catalog.datasets
WHERE EXISTS (
  SELECT FROM unnest(ARRAY['view_roles','query_roles','download_roles']) AS k,
    LATERAL (SELECT config->'access'->k AS v) AS l
  WHERE CASE
    WHEN v IS NULL THEN false
    WHEN jsonb_typeof(v) <> 'array' THEN true
    WHEN jsonb_array_length(v) > 1000 THEN true
    ELSE EXISTS (
      SELECT FROM jsonb_array_elements(v) AS e
      WHERE CASE
        WHEN jsonb_typeof(e) <> 'string' THEN true
        ELSE char_length(e #>> '{}') NOT BETWEEN 1 AND 128
          OR e #>> '{}' ~ '[\u0001-\u0020\u007f-\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000\ufeff]'
      END)
  END);
```

y repararlas con SQL, con permiso de escritura sobre la tabla, corrigiendo la lista y subiendo la
revisión
(`UPDATE ... SET config = jsonb_set(config, '{access,view_roles}', '[...]'), revision = revision + 1,
updated_at = now() WHERE id = ... AND revision = ...`), o recrearlas: DELETE con la revision del listado
o del ETag del detalle en If-Match y después POST.

Los ejemplos no incluyen tokens reales. Usar cabecera Authorization, nunca query string.

Error uniforme `{code,message,request_id}`; sin SQL, valores filtrados, paths ni credenciales.
Toda respuesta lleva `x-request-id` con formato `<nonce>-<n>` (nonce de 16 hex fijado al
arrancar, contador del proceso); en errores coincide con `request_id` del cuerpo. Se ignora el
`x-request-id` del cliente. Cada petición deja una línea JSON `event: request` en stderr con
request_id, method, path (sin query), status, duration_ms y `code` en errores.
400 invalid_request/invalid_filter; 401 invalid_token; 403 permission_denied;
404 dataset_not_found/not_found; 405 method_not_allowed; 409 publication_conflict/schema_changed;
412 revision_mismatch; 413 query_too_large; 500 internal_error;
503 capacity_exceeded/database_unavailable; 504 query_timeout. Mapeo de la base: `PoolTimedOut` → 503 capacity_exceeded; I/O, TLS,
`PoolClosed` y SQLSTATE clase 08, clase 53, 57P01, 57P02, 57P03 → 503 database_unavailable;
57014 → 504 query_timeout; 23505 en el catálogo → 409 publication_conflict; 42703/42P01 en
consultas de datos → 409 schema_changed; clase 22 en consultas de datos y XX000 solo al preparar
la geometría del cliente → 400 invalid_filter; cualquier otro SQLSTATE (42501, 42883, XX000…) y
errores de decodificación → 500 internal_error, que registra en stderr
`{"event":"database_error","sqlstate":"42501"}` (o `"kind":"decode"`) sin mensaje, SQL ni
valores; el cuerpo HTTP no incluye el SQLSTATE.
not_found es cualquier ruta inexistente (dataset_not_found queda reservado a datasets);
method_not_allowed es una ruta existente con otro método y conserva la cabecera Allow.
Ambos, como /ready en 503 (database_unavailable), usan el error uniforme con
Content-Type: application/json y Cache-Control: no-store. Export sin cupo de export libre
(`export_operations`) responde 503 capacity_exceeded inmediato, antes de cabeceras. /health y /ready no autentican:
cualquier Authorization se ignora. Toda respuesta, incluidas MVT y exportaciones en streaming,
lleva X-Content-Type-Options: nosniff. No convertir resultados truncados en 200. Errores tardíos de stream
interrumpen el cuerpo y se registran; no pueden sustituir headers ya enviados.
Export lee cada lote del cursor fila a fila y descuenta `export_bytes` por feature: al superarlo
deja de leer, corta el cuerpo con query_too_large sin `]}` ni chunk final y libera el cupo; en
memoria nunca hay más que el presupuesto restante más una fila. Una feature cuya codificación
supera `export_bytes` no sale de PostGIS (el cursor la devuelve NULL) y produce el mismo corte.

CORS (`tower_http::cors::CorsLayer`) está deshabilitado por defecto: sin `[cors]` no hay
cabeceras `access-control-*` y `OPTIONS` devuelve 405 method_not_allowed. Con
`allowed_origins` es la capa más externa, antes de autenticación y del log: el preflight no
exige token (ni lleva `x-request-id`, nosniff o línea de log) y los 401/4xx/5xx también llevan
CORS. Métodos `GET, POST, PATCH, DELETE`; cabeceras permitidas `authorization, content-type,
if-match`; expuestas `etag, content-disposition, x-request-id`; `max-age` 600; `vary` incluye
`origin`; nunca `Access-Control-Allow-Credentials`. Un origen no listado no recibe
`access-control-allow-origin`; `["*"]` responde `*`.

`message` es una frase fija en inglés por código, sin datos de la petición:

| code | message |
|---|---|
| invalid_request | The request has an invalid or unsupported parameter, header or body. |
| invalid_filter | The filter is not a valid predicate for this dataset. |
| invalid_token | The bearer token is not valid. |
| permission_denied | The principal is not allowed to perform this operation. |
| dataset_not_found | The dataset does not exist or is not visible. |
| not_found | No route matches this path. |
| method_not_allowed | The method is not allowed for this route. |
| publication_conflict | A publication with this id already exists. |
| schema_changed | The publication no longer matches its source. |
| revision_mismatch | The If-Match revision does not match the current revision. |
| query_too_large | The request exceeds a configured size or result budget. |
| internal_error | Unexpected server error; report the request_id. |
| capacity_exceeded | The server is at capacity; retry later. |
| database_unavailable | The database is unavailable; retry later. |
| query_timeout | The query exceeded its time budget. |

## Ejemplo de selección

```json
{"filter":{"op":"and","args":[{"op":"gte","field":"poblacion","value":100000},{"op":"is_not_null","field":"nombre"}]},"bbox":[-104,20,-102,22],"properties":["nombre"],"limit":100,"offset":0}
```

properties solo admite nombres publicados. El id va siempre en `Feature.id` y no necesita
estar publicado ni pedirse en properties.

La misma semántica se prueba en items/query/export/MVT con los límites propios de cada salida.
La validación de un tile compara entidades codificables tras clipping/cuantización; un tile
no demuestra exportación íntegra del dataset. No hay una promesa de conformidad OGC.
