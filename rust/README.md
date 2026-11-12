# Atlas Rust: arranque inicial (#211)

Selección espacial (#222): `spatial::prepare` valida topología y transforma únicamente
la selección 4326 al CRS nativo, dentro de una transacción con timeout. No repara
geometrías. La selección exige XY en el dominio geográfico; el predicado combina
`&&` y la relación exacta sobre la columna nativa. EXPLAIN prueba elegibilidad GIST,
sin garantizar el plan elegido en producción ni latencia.

`selection::prepare(tx, fixed, user, bbox, metadata, limits)` es la entrada común
para combinar restricciones tras autorizar: interpreta los filtros con presupuestos,
valida atributos y topología, y devuelve `Selection::append` para el WHERE SQLx.
Usar la misma transacción de solo lectura al seleccionar filas. Conserva AND entre
publicación, usuario y bbox incluso con OR/NOT anidados. Sin nuevas rutas HTTP.

El parser valida los presupuestos configurados y rechaza objetos usados como escalares,
incluido el marcador interno de números de Serde: preservar numeric no permite coerción
de objetos JSON a números. Las pruebas incluyen este caso ofensivo en comparación e IN.

Desde `rust/` (también en CI), rustup lee `rust-toolchain.toml`: Rust 1.99.1,
estable al iniciar esta implementación. `Cargo.lock` fija la resolución.
Dependencias y features del motor aprobadas por ADR 0008; Serde/TOML y SQLx
se incorporan con sus componentes, sin habilitar drivers ajenos a PostgreSQL.
No se modifican dependencias ni publicación Python.

JWT usa `jsonwebtoken` 11.1 con PEM y AWS-LC (ADR 0008), sin features por defecto.
El proveedor compila código nativo y requiere compilador C/CMake en builds limpios;
Cargo.lock fija también sus dependencias transitivas. No implementamos criptografía propia.

```bash
cd rust
cargo build --locked
cargo run --locked -- serve --listen 127.0.0.1:8080
```

Configuración inicial: `serve` acepta únicamente `--listen IP:PORT`; por defecto
127.0.0.1:8080. Puerto 0 permite asignación del sistema; el evento `listening`
informa la dirección efectiva. Argumentos inválidos o fallos de bind devuelven
salida no cero con mensajes fijos, sin copiar argumentos ni errores internos.
Configuración de dominio/archivo, límites y errores REST pertenecen a #212.

`GET /api/v1/health` devuelve 200; `/api/v1/ready` comprueba pool y acceso a las
columnas del catálogo: 200 si están disponibles, 503 estructurado si no.
Sin DSN se puede iniciar health, pero readiness permanece en 503. El resto
devuelve 404; aún no hay API geoespacial. SIGINT y,
en Unix, SIGTERM detienen aceptación y esperan conexiones mediante el shutdown
de Axum. Logs JSON en stderr (`api::log`): el middleware `api::track` emite una línea
`{"event":"request","request_id","method","path","status","duration_ms"}` por petición, con
`code` si es error; `path` sin query y truncado a 256 bytes, nunca cabeceras, cuerpos ni tokens.
El cierre del servidor espera las conexiones prestadas y cierra el pool.

## Contenedor de desarrollo

Sin Rust instalado en el host: el perfil `rust` de `compose.yaml` construye `rust/Dockerfile`
(toolchain fijado, CMake para AWS-LC y dependencias de `Cargo.lock` ya descargadas) y monta el
repositorio en `/work`. Cargo corre sin red; si cambia `Cargo.lock`, reconstruir la imagen.
`postgis-rust` es una BD descartable en tmpfs para `ATLAS_TEST_DATABASE_URL`, como en CI;
`ATLAS_DATABASE_URL` apunta al `postgis` de desarrollo con `scripts/seed.sql`.
Corre con el uid/gid `ATLAS_UID`/`ATLAS_GID` y deja `target` en un volumen. El `Makefile` de la
raíz los toma del usuario actual y agrupa los comandos:

| Comando | Qué hace |
|---|---|
| `make build` | Construye la imagen de desarrollo |
| `make check` | `fmt`, `lint` y `test`, los mismos pasos que CI |
| `make fmt` / `make lint` / `make test` | Cada paso por separado |
| `make migrate` | Crea el catálogo en el `postgis` de desarrollo |
| `make up` | Levanta el servicio `atlas` en segundo plano: migra y sirve en `127.0.0.1:${ATLAS_PORT:-8080}` |
| `make serve` | Lo mismo en primer plano |
| `make logs` / `make restart` | Sigue los logs / reinicia y recompila |
| `make down` | Detiene los contenedores sin borrar volúmenes |
| `make shell` | Bash dentro del contenedor |
| `make backup` | `pg_dump` del `postgis` de desarrollo a `backups/<bd>-<fecha>.dump` (ignorado por git) |
| `make restore` | Restaura el respaldo más reciente, o `FILE=ruta.dump`, en una sola transacción |

`rust` es el servicio de herramientas y tests; `atlas` es el servidor. La BD de tests es
descartable y no se respalda.

## Configuración y dominio (#212)

`serve --config archivo.toml` admite `listen`, `database_url`, `allowed_schemas`
y tabla `[limits]` con los defaults de spec.md. No acepta claves desconocidas.
`allowed_schemas` rechaza `atlas_catalog`, `information_schema` y todo prefijo `pg_`
(`pg_catalog`, `pg_toast`, `pg_temp_N`); `serve` termina con `startup_failed`.
`ATLAS_DATABASE_URL` y `ATLAS_LISTEN` prevalecen sobre TOML; `--listen` prevalece
sobre el entorno. DSN nunca pertenece a un Dataset ni se imprime al fallar.
La configuración se valida antes del bind/pool. No derive Debug/Serialize en Config.
El DSN es opcional solo para health; para operar datos es obligatorio.

Domain no importa HTTP/SQL: Dataset/Source/AccessPolicy/Principal, Query y Error.
Después de deserializar se llama `validate` con schemas/límites del servidor;
la introspección posterior verifica propiedades y tipos reales. Los filtros se
conservan como RawValue sin redondear numeric: su parser acotado y validación
semántica se implementan en #220; `selection::prepare` los valida antes de consultar.

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

El smoke Unix ejecuta el binario, verifica HTTP real, rechazo de configuración
y puerto ocupado, SIGTERM/SIGINT y salida limpia con plazo. No demuestra aún
cancelación de consultas/exports ni rendimiento; esas pruebas llegarán con
sus componentes. El job Rust de CI es independiente del job Python existente.

## Pool PostGIS (#213)

`Database::open` valida configuración y extensión PostGIS; máximo 10 conexiones,
espera 5 s por defecto. `begin_read` abre REPEATABLE READ READ ONLY y
`begin_catalog` abre READ WRITE; ambas aplican statement_timeout local (30 s).
SQLx revierte transacciones descartadas antes de reutilizar la conexión. Cancelar
el future no garantiza cancelación SQL inmediata: el timeout del servidor acota
el trabajo restante. No registrar SQL/DSN/errores del driver. `close` espera leases.
Pruebas reales requieren `ATLAS_TEST_DATABASE_URL` hacia una BD descartable con
PostGIS; fallan si falta. CI proporciona una BD separada para Rust.

## Introspección (#214)

`postgis::introspection::inspect` exige schemas permitidos y propiedades explícitas.
Tablas requieren PK simple; vistas/materializadas, id declarado único/no nulo.
Una geometría se infiere solo si es única; varias requieren columna explícita.
Geometría genérica vacía se rechaza; tipada vacía conserva CRS/tipo. Toda geometría
se excluye de atributos. Los nombres se resuelven por parámetros en pg_catalog y
se citan como identificadores al consultar la relación. Pruebas contra PostGIS real.
Los tipos se comparan por OID con los tipos base de pg_catalog (int2…timestamptz): dominios,
enums y homónimos de otros esquemas son 400; la geometría debe ser `geometry` del esquema de la
extensión PostGIS. POST/PATCH exigen SELECT del rol actual (`has_column_privilege`) sobre
geometría, id y cada propiedad publicada; si falta, 400 sin indicar la columna.

## Migración explícita (#215)

`atlas migrate --config ddl.toml` crea `atlas_catalog.datasets` en una transacción
con bloqueo de migración. Repetirla es seguro; `serve` nunca ejecuta DDL.
Usar un rol DDL separado: propietario del esquema/tabla, con CREATE sobre la base.
`CREATE SCHEMA IF NOT EXISTS` exige ese permiso incluso si `atlas_catalog` ya existe.
La cuenta de servicio recibe únicamente USAGE en atlas_catalog y
SELECT/INSERT/UPDATE/DELETE en datasets, además de USAGE/SELECT en los
esquemas/relaciones fuente permitidos. Nunca concederle CREATE ni propiedad de
catálogo, base o fuentes.

Ambos roles necesitan USAGE sobre el esquema donde está instalada la extensión
PostGIS, también cuando `public` fue revocado a PUBLIC. Antes de migrar o servir,
el propietario de la base configura los permisos; nombres de ejemplo:

```sql
GRANT CREATE ON DATABASE atlas TO atlas_ddl;
-- Si PostGIS está instalado en public:
GRANT USAGE ON SCHEMA public TO atlas_ddl, atlas_service;
-- Confirmar la ubicación real de la extensión:
SELECT extnamespace::regnamespace FROM pg_extension WHERE extname = 'postgis';
```

Si PostGIS está en otro esquema, sustituir `public` en el GRANT e incluir ese esquema
en el `search_path` de ambos roles: Atlas llama funciones como `postgis_version()` y
`ST_Transform` sin prefijo de esquema. USAGE permite resolver la extensión, no concede
SELECT sobre fuentes ni modifica `allowed_schemas`. Por ejemplo, con PostGIS en `postgis`
(el catálogo y las fuentes se califican con esquema y no dependen del `search_path`):

```sql
GRANT USAGE ON SCHEMA postgis TO atlas_ddl, atlas_service;
ALTER ROLE atlas_ddl SET search_path = postgis;
ALTER ROLE atlas_service SET search_path = postgis;
```

PostGIS debe estar instalado de antemano por el administrador. Sin estos permisos,
`migrate` puede fallar con `migration failed` y `serve` con `database_unavailable`; los errores públicos omiten
los detalles internos deliberadamente. Las pruebas de permisos usan PostGIS descartable;
no modifican roles ni fuentes de ningún despliegue existente.

`Catalog` valida configuración y metadatos al leer, vuelve a inspeccionar al crear
o reemplazar y no usa caché. Update/delete comparan revisión en el mismo statement;
una revisión obsoleta devuelve RevisionMismatch. Borrar retira solo publicación,
nunca datos fuente. El repositorio recibe configuración completa: Merge Patch y
ETag de transporte se incorporan en #218; el filtro opaco se valida en #220 antes
de habilitar publicación por HTTP. No hay DDL automático en operaciones CRUD.

## Identidad (#216)

`auth::jwt::Verifier` verifica únicamente RS256 con PEM público local, issuer y audience
configurados. Exige sub no vacío y exp futuro, valida nbf cuando existe, sin tolerancia
de reloj; roles omitidos producen lista vacía y roles malformados se rechazan.
Claims externos adicionales se permiten; jku/kid no seleccionan claves ni disparan red.
Tokens de más de 16 KiB se rechazan. Claves en tests/fixtures son ficticias y públicas;
jamás usarlas en despliegues. No hay emisor, login ni OIDC.

TOML admite `[auth]` opcional; al incluirlo exige `public_key_file`, `issuer` y `audience`.
Rutas relativas se resuelven desde el directorio de trabajo del proceso.
El PEM se carga una vez antes del listener; rotarlo requiere reiniciar.
La clave (SPKI o PKCS#1) debe ser RSA de 2048 a 8192 bits con exponente impar ≥ 3;
si no, `serve` termina con `startup_failed` sin abrir el puerto.
`leeway_seconds` (opcional, 0..=300, por defecto 30) tolera desfase de reloj en `exp`/`nbf`.

```toml
[auth]
public_key_file = '/etc/atlas/jwt-public.pem'
issuer = 'https://identity.example.test'
audience = 'atlas'
leeway_seconds = 30
```

El middleware verifica Authorization antes de ejecutar handlers, incluidas rutas públicas.
Sin cabecera inserta un Principal anónimo; tokens inválidos devuelven 401 estructurado.
Con auth sin configurar, cualquier Authorization falla cerrado. Solo Bearer en cabecera;
credenciales en query string no se usan. No registrar tokens ni cabeceras. Cada 401
registra `auth_failed` con `request_id` y un `reason` estable (ver auth-verification.md).

CORS (#342) deshabilitado por defecto; `[cors] allowed_origins` admite hasta 100 orígenes
únicos, `*` solo o `http(s)://host[:puerto]`; si no, `startup_failed`. Cabeceras y orden de
capas: `docs/rust/api.md`.

## Políticas (#217)

`auth::policy::{view,query,download,admin}` son funciones sin I/O: invocarlas antes de
acceder a datos. Privado deniega por defecto; query/download exigen view. Public concede
view/query; public_download concede descarga pública. Invisible/deshabilitado devuelve
DatasetNotFound; operación faltante sobre visible, PermissionDenied. atlas-admin permite
solo administración, sin bypass de datos; roles de un Principal anónimo no conceden acceso.
El listado filtra por view antes de paginar. Los filtros de datos nunca conceden permisos.
Integración de estas funciones en rutas administrativas/datos: #218–#219 y siguientes.

## Filtros (#220)

`query::predicate::parse` convierte el filtro JSON en un `Predicate` cerrado antes de tocar SQL.
Mide bytes antes de parsear; rechaza claves duplicadas, desconocidas o faltantes, operadores
desconocidos, valores que no sean texto, booleano o número finito, y `field` que no sea
identificador. Numeric se conserva como `serde_json::Number`, sin redondear. `in` exige de 1 a
`in_values` escalares. Todo rechazo es `invalid_filter`; tipos contra columnas y SQL: #221.
`and`/`or`/`not` cuentan profundidad y nodos antes de descender, así que la recursión queda
acotada por `filter_depth` (máximo 128); un JSON anidado sin límite falla en serde_json sin
desbordar la pila.
`intersects`/`within` validan solo la estructura de Polygon/MultiPolygon: anillos cerrados de al
menos cuatro posiciones XY finitas y `xy_pairs` en todo el filtro. La validez topológica se
comprueba en PostGIS (#222); no hay motor geométrico en Rust.

## Filtros en SQL (#221)

`postgis::predicates::compile` escribe un `Predicate` en un `QueryBuilder`. Solo admite atributos
publicados según la introspección, con identificadores citados y valores enlazados como texto con
cast explícito al tipo de la columna (`$n::int8`, `$n::numeric`…), así que bigint y numeric no
pasan por f64. Tipos estrictos: enteros dentro del rango de su columna, `like`/`ilike` solo en
texto, fechas ISO. Bbox y selecciones espaciales: #222.
`restrict` une con AND el filtro fijo de la publicación y el del usuario, cada uno entre
paréntesis: ningún `or`/`not` del usuario amplía lo publicado; sin filtros escribe `TRUE`.
`query_error` traduce los errores de datos de PostgreSQL (SQLSTATE 22, p. ej. 2026-02-30) a
`invalid_filter`.

## Publicaciones (#218)

`services::publications::Publications` valida antes de tocar el catálogo: JSON cerrado (claves
duplicadas o desconocidas son `invalid_request`), `Dataset::validate`, introspección de la fuente
(inexistente o fuera de `allowed_schemas` es `invalid_request`) y el filtro fijo preparado con
`selection::prepare`, que lo compila contra los atributos publicados y valida su geometría en
PostGIS. `list` ordena por id con `catalog_default`/`catalog_max`; `document` arma la respuesta
administrativa (`config`, `revision` y `metadata` pública). Borrar conserva la tabla.
`update` aplica Merge Patch (RFC 7386) sobre la configuración vigente: lo omitido se conserva,
`null` borra opcionales, `id` no se acepta y `filter` se reemplaza entero. Las claves duplicadas se
rechazan también dentro del parche. Una revisión distinta es `revision_mismatch` antes de
reinspeccionar; la configuración resultante se valida como en `create`.
PATCH conserva el filtro como JSON original hasta validarlo: un objeto nunca se
normaliza a número mediante el marcador interno de Serde y numeric mantiene precisión.

## Rutas administrativas (#218)

`api::admin::router` monta `/api/v1/admin/datasets` y `/{id}`; exige `atlas-admin` (403) y responde `no-store`.
Detalle con ETag; listado con `limit`/`offset` únicos; errores `{code,message,request_id}`.
POST, GET y PATCH devuelven el ETag; PATCH y DELETE exigen un único `If-Match` con la revisión
entre comillas (ausente o malformado 400, distinta 412) y DELETE responde 204. Cuerpos mayores
que `admin_body_bytes` son 413 `query_too_large`.

## Descubrimiento (#219)

`Publications::visible` resuelve el catálogo vigente en cada llamada, sin caché, y filtra por
`view` antes de paginar por id (`catalog_default`/`catalog_max`), así que totales y `hasMore` no
delatan datasets ocultos; las publicaciones que ya no validan se omiten (`Catalog::servable`).
`servable` recorre el catálogo por lotes de 256 con clave (`id > $1`), aplica el filtro dentro del
recorrido y para en cuanto tiene `offset + limit + 1` visibles: la memoria queda acotada, aunque un
catálogo casi todo privado se recorre casi entero.
`detail` exige `view` (si no, 404) y `query` (si no, 403); una publicación que no valida también es
404. `summary` es la vista pública: sin fuente, filtro fijo, roles ni revisión.
`GET /api/v1/datasets` y `/{id}` exponen `visible` y `detail` con `no-store`. Un cambio
administrativo se ve en la siguiente petición; quitar un rol de la política corta el acceso ahí
mismo aunque el token ya emitido siga trayéndolo.
## MVT nativo (#225)

`postgis::mvt::tile` codifica en PostGIS con `ST_AsMVTGeom`/`ST_AsMVT` sobre la `Selection`
preparada; Rust no materializa geometrías. Toma `tile_candidates + 1` candidatos dentro del sobre
de la tesela más el buffer (recortado a ±H en los bordes del mundo; en z=0 un SRID local no usa
sobre), y mide `octet_length` dentro de la base: si hay más candidatos o bytes que el presupuesto,
`query_too_large` sin transferir la tesela; nunca un LIMIT parcial silencioso. Una tesela sin
entidades es un `Vec` vacío. Numeric y fechas viajan como texto; floats NaN/infinitos se omiten;
el id de la entidad solo sale de ids enteros entre 0 y 2^53−1.
`tests/postgis_mvt.rs` decodifica las teselas con un lector protobuf propio, independiente de
PostGIS, para comprobar ids, atributos, filtros, bordes del mundo, SRID local y mosaicos.

## Teselas (#226)

`GET /api/v1/datasets/{id}/tiles/{z}/{x}/{y}.mvt` valida id, coordenadas, zoom (`tile_zoom`) y la
query (`filter` como JSON codificado y `properties` separadas por comas, una vez cada uno) antes de
consultar la base. Exige `view` y `query`, aplica el mismo predicado que GeoJSON (filtro fijo AND
filtro del usuario) y responde `application/vnd.mapbox-vector-tile` con `no-store`; sin entidades,
200 con cuerpo vacío.
`GET /api/v1/datasets/{id}/tilejson.json` (TileJSON 3.0.0, mismas reglas de acceso) describe la
capa con el id del dataset como `source-layer` y solo los campos publicados (`String`, `Number`,
`Boolean`). La URL de teselas sale de `public_url` en TOML, que admite prefijo de despliegue
(`https://host/prefijo`, sin barra final, query ni fragmento); sin ella es relativa. Las cabeceras
`Forwarded`/`X-Forwarded-*` se ignoran.
## Páginas de features (#223)

`postgis::features::page` recibe la `Selection` preparada en la misma transacción de lectura y
arma cada Feature en PostgreSQL con `json_build_object`: `id` siempre presente, geometría con
`ST_AsGeoJSON` en EPSG:4326 y solo las propiedades publicadas pedidas (por defecto todas). Las
filas llegan como texto JSON y se conservan como `RawValue`, así que bigint y numeric no pasan por
f64. El orden pedido se valida contra atributos publicados (o el id) y el id desempata. Pide
`limit + 1` para `more`, sin COUNT; offset no es un snapshot entre peticiones.

## Consultas GeoJSON (#224)

`GET /api/v1/datasets/{id}/items` arma un `Query` con `filter` (JSON codificado), `bbox` y
`properties` por comas, `sort` (`campo:asc,otro:desc`), `limit` y `offset`. Exige `view` y `query`
y obtiene la página completa antes de enviar cabeceras: un error nunca sale como 200. Responde
`application/geo+json` con `no-store`, `numberReturned` y `hasMore`; el filtro fijo siempre se
combina con el del usuario.
`POST /api/v1/datasets/{id}/query` recibe el mismo `Query` como JSON cerrado y produce la misma
selección: claves desconocidas o duplicadas 400, cuerpo mayor que `query_body_bytes` 413. En GET,
parámetros desconocidos o repetidos son 400.

## Cursor de exportación (#302)

`exports::geojson::Export` cuenta hasta `export_features + 1` y declara un cursor en
la misma transacción READ ONLY REPEATABLE READ. Reutiliza selección y codificación
GeoJSON, lee lotes de 128 y no aplica `default_limit`. Cada chunk consume el presupuesto
de bytes y el deadline total; un error tardío impide completar la colección. Finalizar o
abandonar el cursor revierte la transacción antes de devolver su conexión al pool.
La ruta y los permisos de descarga se incorporan en #303.

## Descarga GeoJSON (#227)

`GET /api/v1/datasets/{id}/export` exige view+download (no query), acepta únicamente
`format=geojson`, `filter`, `bbox` y `properties`, únicos. Comparte el filtro fijo AND
usuario AND bbox con items. Devuelve `application/geo+json`, `no-store` y attachment
con nombre derivado del id validado. Conteo excedido: 413 antes de headers. Presupuestos
tardíos: corta el cuerpo y registra una sola línea `export_failed` con `code` fijo y el
`request_id` de la petición que abrió la descarga, sin datos del usuario.
El cliente debe tratar cualquier colección incompleta como una descarga fallida.

## Admisión y cancelación (#228)

Un semáforo compartido limita `data_operations` (8 por defecto) para features, MVT y
exportaciones; adquisición inmediata o 503. Catálogo, administración y health no usan cupos.
El deadline de respuesta no cancela el worker SQL: conserva el cupo hasta rollback real;
el timeout PostgreSQL acota el drenaje. Export transfiere el cupo al cursor y un productor
con canal acotado aplica `stream_seconds` incluso si el cliente deja de leer. El pool se
cierra después del drenaje del servidor; no se interpreta el rechazo como throughput.

## Ejecutar el MVP experimental (#229)

La imagen `runtime` contiene solo el binario release y bibliotecas de sistema, sin Cargo,
fuentes ni claves. Corre como uid/gid 65532. El target `dev` y el perfil Compose `rust`
siguen disponibles; no se publica una imagen ni se despliega automáticamente.

```bash
docker build --target runtime -t atlas-rust:experimental rust
docker run --rm --network host --env-file /ruta/atlas-ddl.env \
  -v /ruta/atlas.toml:/etc/atlas.toml:ro atlas-rust:experimental migrate --config /etc/atlas.toml
docker run --rm --name atlas-rust --network host --env-file /ruta/atlas-service.env \
  -v /ruta/atlas.toml:/etc/atlas.toml:ro \
  -v /ruta/public.pem:/etc/atlas/public.pem:ro \
  atlas-rust:experimental serve --config /etc/atlas.toml --listen 127.0.0.1:8081
curl -fsS http://127.0.0.1:8081/api/v1/ready
docker stop --timeout 360 atlas-rust
```

Los env-files contienen `ATLAS_DATABASE_URL`: rol DDL solo al migrar y rol de servicio
limitado al servir (permisos descritos arriba). `atlas.toml` define `allowed_schemas` y
`[auth]` con `public_key_file`, `issuer` y `audience`; la clave pública debe ser legible por
65532. Nunca montar una clave privada. SIGTERM deja de aceptar y drena antes de cerrar
el pool; el plazo del supervisor debe cubrir consultas y streams configurados.
Ver [matriz de aceptación](../docs/rust/acceptance.md). El MVP no demuestra aptitud de
producción ni reemplazo completo de GeoServer; no migra consumidores ni retira Python.

El presupuesto `export_bytes` incluye toda la colección GeoJSON, también los dos bytes
finales `]}`. El cursor los reserva antes de emitir: un presupuesto igual al tamaño total
permite completar la descarga; si falta un byte, falla antes de emitir el último chunk
que ya no deja espacio para el cierre, sin superar el límite ni simular éxito parcial.

La forma y los presupuestos del filtro se validan antes de resolver el dataset, como
en items/query: un filtro malformado devuelve 400 incluso si el id no existe. Para
consultas válidas, export resuelve y autoriza view+download antes de adquirir un cupo;
una denegación conserva su 403/404 aunque todos los cupos estén ocupados.
