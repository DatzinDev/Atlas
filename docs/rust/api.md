# Contrato REST del MVP

Especificación futura [OpenAPI 3.0.3](openapi.yaml); implementación pendiente. Requisitos [R01–R17](spec.md).
Prefijo fijo de versión `/api/v1`; los clientes no usan rutas Python como aliases.

## Transporte y coherencia

Query es idéntico para GET items y POST query. filter en GET es JSON codificado en URL;
bbox y properties usan listas separadas por comas; sort usa `field:asc,other:desc`.
GET admin/listado solo admite limit/offset. Export acepta filter/bbox/properties;
tiles solo filter/properties porque XYZ define su ventana. Parámetros desconocidos o repetidos
y claves JSON duplicadas son 400, no se ignoran. No aceptar ATLAS_FILTER, CQL o SQL del cliente.
POST query no crea recursos ni persiste consultas. GET/POST producen la misma selección.
Todas las respuestas de datos dinámicas tienen Cache-Control: no-store; no hay caché MVT inicial.
GeoJSON EPSG:4326; tiles XYZ/MVT EPSG:3857. TileJSON 3.0 incluye URL, source-layer,
minzoom/maxzoom, bounds y attribution disponibles. Las URLs respetan el prefijo del despliegue
configurado; no se confía en cabeceras forwarded arbitrarias.

El catálogo público describe propiedades publicadas; jamás devuelve source, filter fijo,
roles, SQL o credenciales. El administrativo sí devuelve configuración sin credenciales.
PATCH usa Merge Patch: campos omitidos se conservan; null solo borra campos opcionales;
la configuración resultante se valida e inspecciona como en POST. id no cambia con PATCH.
Publicar relación ambigua exige geometry_column explícita. id_column es PK simple o clave
de vista única/no nula verificada. Source solo referencia la BD configurada.
ETag es la revision entre comillas en detalle administrativo/POST/PATCH. If-Match es obligatorio
para PATCH/DELETE; ausente o malformado es 400, revisión distinta 412. DELETE devuelve 204.
No usar ETag de datos: la revisión del catálogo no identifica cambios externos en PostGIS.

## Autorización y errores

Bearer JWT RS256 emitido externamente, sub/iss/aud/exp obligatorios y roles array de strings.
Sin token: Principal anónimo. Si hay token, siempre verificarlo antes de aplicar acceso público.
Solo atlas-admin usa /admin; no equivale a autorización de query/download.
Catálogo filtra por view; detalle/items/tiles exigen view+query; export view+download.
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
