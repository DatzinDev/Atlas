# Contrato REST del MVP

Contrato del MVP experimental [OpenAPI 3.0.3](openapi.yaml); evidencia en [aceptación](acceptance.md). Requisitos [R01–R17](spec.md).
Prefijo fijo de versión `/api/v1`; los clientes no usan rutas Python como aliases.

## Transporte y coherencia

Query es idéntico para GET items y POST query. filter en GET es JSON codificado en URL;
bbox y properties usan listas separadas por comas; sort usa `field:asc,other:desc`.
GET admin/listado solo admite limit/offset. Export acepta filter/bbox/properties;
tiles solo filter/properties porque XYZ define su ventana.
bbox es `minx,miny,maxx,maxy` en EPSG:4326 con `-180 ≤ minx < maxx ≤ 180` y
`-90 ≤ miny < maxy ≤ 90`; fuera de eso (incluido área cero) es 400 invalid_request.
Un bbox que cruza el antimeridiano (`minx > maxx`) se rechaza: el cliente parte la consulta
en dos. Todo error causado por el bbox es invalid_request; los de geometrías de filter son
invalid_filter. En datasets EPSG:3857 bbox y polígonos se recortan a latitud
±85.0511287798066 (no es error; un polígono sin parte dentro no selecciona filas). Si el CRS
nativo no es 4326 se densifican cada 1° antes de transformar; una selección transformada
inválida, vacía o sin área es 400, nunca un 200 casi vacío. Parámetros desconocidos o repetidos
y claves JSON duplicadas son 400, no se ignoran. No aceptar ATLAS_FILTER, CQL o SQL del cliente.
POST query no crea recursos ni persiste consultas. GET/POST producen la misma selección.
Todas las respuestas de datos dinámicas tienen Cache-Control: no-store; no hay caché MVT inicial.
GeoJSON EPSG:4326; tiles XYZ/MVT EPSG:3857. TileJSON 3.0 incluye URL, source-layer,
minzoom/maxzoom, bounds y attribution disponibles. Las URLs respetan el prefijo del despliegue
configurado; no se confía en cabeceras forwarded arbitrarias.
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
PATCH usa Merge Patch: campos omitidos se conservan; null solo borra campos opcionales;
la configuración resultante se valida e inspecciona como en POST. id no cambia con PATCH.
Publicar relación ambigua exige geometry_column explícita. id_column es PK simple o clave
de vista única/no nula verificada. Source solo referencia la BD configurada.
Tipo y SRID salen del typmod; si falta alguno, se infiere recorriendo todas las filas no nulas
(sujeto a statement_timeout, exceso 504 query_timeout). Se exige un único SRID existente en
spatial_ref_sys, solo 2D (Z/M es 400) y un único tipo o exactamente X+MultiX (POINT, LINESTRING,
POLYGON), publicado como MultiX; otra mezcla es 400. Una relación sin filas no nulas y sin typmod
es 400: declarar la columna como geometry(Tipo,SRID).
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
Catálogo filtra por view; detalle/items/tiles exigen view+query; export view+download.
view_roles, query_roles y download_roles admiten hasta 1.000 roles cada una; cada rol tiene
de 1 a 128 caracteres sin espacios (incluido U+FEFF) ni caracteres de control. Fuera de eso
es 400 invalid_request al publicar o con PATCH. Si se quita el rol de la publicación, el token
que aún lo traiga ya no autoriza desde la siguiente petición, porque no hay caché de catálogo;
un stream ya autorizado sigue hasta terminar. Si el emisor deja de otorgar el rol, el token
emitido lo conserva hasta exp; esa ventana la fija el emisor.

Una publicación guardada con roles fuera de esa regla, o con una lista que no es un arreglo de
strings, deja de leerse: su detalle administrativo responde 409 schema_changed, igual que cada
página del listado administrativo que la incluya; PATCH también (lee antes de escribir) y DELETE
se queda sin ETag. En las rutas públicas no hay 409: la publicación desaparece del catálogo y su
detalle es 404. Antes de desplegar, detectarlas con un SELECT de solo lectura, que solo requiere
SELECT sobre atlas_catalog.datasets (no el rol DDL):

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
updated_at = now() WHERE id = ... AND revision = ...`), o recrearlas: DELETE con la revision leída
de la tabla en If-Match y después POST.

Los ejemplos no incluyen tokens reales. Usar cabecera Authorization, nunca query string.

Error uniforme `{code,message,request_id}`; sin SQL, valores filtrados, paths ni credenciales.
400 invalid_request/invalid_filter; 401 invalid_token; 403 permission_denied;
404 dataset_not_found; 409 publication_conflict/schema_changed; 412 revision_mismatch;
413 query_too_large; 500 internal_error; 503 capacity_exceeded/database_unavailable;
504 query_timeout. No convertir resultados truncados en 200. Errores tardíos de stream
interrumpen el cuerpo y se registran; no pueden sustituir headers ya enviados.

## Ejemplo de selección

```json
{"filter":{"op":"and","args":[{"op":"gte","field":"poblacion","value":100000},{"op":"is_not_null","field":"nombre"}]},"bbox":[-104,20,-102,22],"properties":["id","nombre"],"limit":100,"offset":0}
```

La misma semántica se prueba en items/query/export/MVT con los límites propios de cada salida.
La validación de un tile compara entidades codificables tras clipping/cuantización; un tile
no demuestra exportación íntegra del dataset. No hay una promesa de conformidad OGC.
