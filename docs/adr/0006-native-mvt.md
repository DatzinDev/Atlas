# ADR 0006 — MVT nativo en PostGIS

**Estado:** propuesto para revisión; implementación pendiente (#106, #110, #128–#140).
**Fecha:** 2026-10-19

## Decisión

PostGIS produce MVT; Atlas valida, autoriza y entrega bytes. No intervienen Pillow,
Shapely para codificación, renderer de servidor ni servicio externo. Se requiere PostGIS
>=3.1 con ST_TileEnvelope, ST_AsMVTGeom, ST_AsMVT y soporte protobuf; la implementación
verificará disponibilidad al habilitar MVT y dará error Atlas claro si falta.
Se conserva el entorno PostGIS 3.4 del proyecto como referencia de integración.

Grid XYZ en EPSG:3857: origen noroeste, z entero 0..22, x/y enteros 0..2**z-1,
256 píxeles lógicos por lado y mundo ±20037508.342789244 metros. Rechazar TMS,
coordenadas fuera del grid y wrap implícito; clientes normalizan x antes de pedir.
No se publica un servicio EPSG:900913 ni WMTS por usar este grid.

Defaults AtlasConfig futuros: mvt_extent=4096, mvt_buffer=64 unidades MVT,
max_tile_bytes=8388608 y max_tile_features=100000. Valores positivos salvo buffer >=0;
extent/buffer se validan y no se permiten overrides HTTP de presupuestos.
El bbox de candidatos incluye buffer/extent; ST_AsMVTGeom recibe bounds sin expandir
con clip_geom=true. El filtro espacial actúa sobre geometría nativa indexada y la
proyección 3857 solo se aplica a candidatos. Predicados fijos y de usuario se combinan
según ADR 0005, con statement_timeout independiente.

## Contrato Python y HTTP

API propuesta: `async get_vector_tile(ctx, name, tile, *, properties=None,
predicate=None, principal=None) -> bytes`; tile es TileCoord validado por core y
predicate es el modelo core, no SQL/JSON libre.
`GET /tiles/{name}/{z}/{x}/{y}.mvt` en el router Atlas, con properties como lista CSV
sin duplicados y ATLAS_FILTER según ADR 0005. No admite STYLES/SLD/CQL_FILTER.
Una sola capa MVT, nombrada con el nombre registrado de Atlas.
MVT se habilita explícitamente por capa (por defecto deshabilitado), requiere tanto
view como query y respeta enabled y whitelist antes de consultar. No hereda permiso
por solo estar publicado en WMS. No existe token o checker propio de tiles.

Éxito: 200, application/vnd.mapbox-vector-tile, bytes sin compresión interna;
sin features codificables: 200 con cuerpo vacío válido para decoder. Errores JSON
Atlas: InvalidParameter/InvalidFilter 400, acceso denegado 403, LayerNotFound 404,
QueryTooLarge 413, QueryTimeout 504. Servicios deshabilitados se tratan como no disponibles.
Una feature fuera de permisos nunca cuenta como un tile vacío exitoso.
Caché inicial deshabilitada, Cache-Control: no-store; sin ETag, segundo backend,
seeding ni invalidación nueva antes de medir.

## Propiedades e identidad

properties ausente selecciona atributos portables publicados; explícito vacío, duplicados,
geometría o columnas no publicadas se rechazan. Escalares soportados: texto, booleano,
int16/32/64 y float finito; NULL se omite. Fechas/timestamps se convierten a ISO-8601
sin inventar zona horaria. numeric/decimal conserva precisión como texto decimal.
JSON/arrays/binarios y tipos sin conversión definida quedan fuera por defecto;
si se solicitan explícitamente, InvalidParameter, nunca stringificación arbitraria.
La PK solo se codifica como feature ID si es un entero no negativo <=2**53-1
compatible con clientes JavaScript. PK ausente, compuesta, textual, negativa o mayor:
sin feature ID; el atributo portable puede conservarse. No se inventan IDs hash.
No equivale al ID string de GeoJSON ni garantiza IDs globales entre capas.

## Integridad y límites

Contar candidatos filtrados hasta max_features+1 dentro de la misma sentencia/snapshot
que genera el tile. Si excede, no agregar MVT parcial. Medir octet_length del resultado
en SQL y devolver solo señal de exceso si supera max_bytes, sin transferir el blob.
La agregación puede consumir memoria de PostgreSQL antes del control de bytes: el
presupuesto de candidatos y statement_timeout reducen el riesgo, no garantizan RSS.
No usar LIMIT silencioso ni samplear. NULL/EMPTY se excluyen; geometrías degeneradas
tras cuantización pueden desaparecer o cambiar dimensión según PostGIS. Probarlo y
reportarlo como semántica MVT, sin llamar a ese resultado descarga íntegra.
Buffer permite duplicados entre tiles adyacentes; comparar cobertura con tolerancia
de cuantización, no igualdad WKB. Geometrías que cruzan antimeridiano deben estar
normalizadas por el host; no se divide ni repara automáticamente su topología.

## Verificación y alternativas

Decoder solo dev: mapbox-vector-tile==2.2.0, instalación/import verificados el
2026-10-19 en Linux x86_64, Python 3.11.14 y 3.13.11; integración PostGIS pendiente.
La matriz de implementación incluirá Python 3.11 y decodificará fixtures de puntos,
líneas, polígonos, bordes, vacío, IDs/propiedades y presupuestos. No añadir el decoder
al runtime. Medir cold/hot, cobertura, bytes, CPU y memoria en #140.
Se descarta encoder Python por copias/coste; se difieren WMTS, TileJSON completo,
GWC REST, estilos cliente, catálogo y seeding. Un tile recortado/cuantizado no sustituye
una descarga completa ni promete servir volumen ilimitado.

Fuentes: [ST_AsMVT](https://postgis.net/docs/ST_AsMVT.html),
[ST_AsMVTGeom](https://postgis.net/docs/ST_AsMVTGeom.html).
