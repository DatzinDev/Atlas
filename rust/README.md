# Atlas Rust: inicio rápido

Servidor HTTP que publica capas PostGIS como catálogo, GeoJSON, MVT/TileJSON y descargas.
**Experimental**: MVP sin garantía de aptitud para producción ni de reemplazo de GeoServer.
Contrato: [spec.md](../docs/rust/spec.md), [api.md](../docs/rust/api.md) y
[OpenAPI](../docs/rust/openapi.yaml).

## Requisitos

- Docker con Compose y `make`. No hace falta Rust en el host: el perfil `rust` de
  `compose.yaml` construye `rust/Dockerfile` (toolchain de `rust-toolchain.toml`).
- `curl` y `openssl` para los ejemplos.
- Sin contenedor: Rust de `rust-toolchain.toml`, compilador C y CMake (AWS-LC), y PostgreSQL
  con PostGIS ya instalado por el administrador.

## Arranque con el entorno de desarrollo

Desde la raíz del repositorio:

| Comando | Qué hace |
|---|---|
| `make up` | Levanta `postgis` (con `scripts/seed.sql`) y `atlas` en segundo plano: migra y sirve en `127.0.0.1:${ATLAS_PORT:-8080}` |
| `make migrate` | Aplica las migraciones de `atlas_catalog` en el `postgis` de desarrollo |
| `make serve` | Igual que `make up`, en primer plano |
| `make logs` / `make down` | Sigue los logs / detiene sin borrar volúmenes |
| `make check` | `fmt`, `lint` y `test`, los mismos pasos que CI |

```bash
make up
curl -fsS http://127.0.0.1:8080/api/v1/health
curl -fsS http://127.0.0.1:8080/api/v1/ready
```

`/ready` responde 200 cuando el pool funciona y el catálogo está en la última migración; si no,
503 `database_unavailable`. `serve` nunca ejecuta DDL: lo hace `migrate`. Ambos exigen
`ATLAS_DATABASE_URL` (o `database_url`).

El servicio `atlas` arranca sin `--config` (solo esquema `public`, sin `[auth]`), así que
rechaza cualquier `Authorization`. Para publicar las capas de `geo` del seed, servir con un
archivo de configuración y la clave pública ficticia de los tests (nunca en despliegues):

```bash
mkdir -p tmp && cat > tmp/atlas-dev.toml <<'EOF'
allowed_schemas = ["geo"]
public_url = "http://127.0.0.1:8081"

[auth]
public_key_file = "tests/fixtures/jwt-test-public.pem"
issuer = "atlas-test"
audience = "atlas"
EOF
docker compose --profile rust run --rm -p 127.0.0.1:8081:8081 rust \
  cargo run --locked -- serve --config /work/tmp/atlas-dev.toml --listen 0.0.0.0:8081
```

`tmp/` está ignorado por git. Las rutas relativas se resuelven desde el directorio de trabajo
del proceso (`/work/rust` en el contenedor).

## Configuración

`atlas serve|migrate [--config FILE] [--listen IP:PORT]`. Las claves desconocidas son error;
`ATLAS_DATABASE_URL` y `ATLAS_LISTEN` prevalecen sobre el TOML y `--listen` sobre el entorno.
Un valor inválido termina con `startup_failed` antes de abrir el puerto. Todas las claves con
su valor por defecto:

```toml
listen = "127.0.0.1:8080"         # puerto 0: lo asigna el sistema (evento listening)
# database_url = "postgresql://…" # sin default; mejor ATLAS_DATABASE_URL
allowed_schemas = ["public"]      # nunca atlas_catalog, information_schema ni pg_*
# public_url = "https://host/prefijo"  # sin default; sin barra final, query ni fragmento

[limits]
default_limit = 1000      # items/query sin limit
max_limit = 10000
max_offset = 100000
catalog_default = 100     # listados de catálogo y admin
catalog_max = 1000
filter_bytes = 65536
filter_depth = 8          # máximo 128
filter_nodes = 100
in_values = 1000
xy_pairs = 10000
pool_min = 1
pool_max = 10             # debe superar data_operations
acquire_seconds = 5
query_seconds = 30        # statement_timeout
stream_seconds = 300      # plazo total de una exportación
shutdown_seconds = 30     # 1..=3600
data_operations = 8       # cupos para items, tiles y export
export_operations = 2     # ≤ data_operations
tile_candidates = 100000
tile_bytes = 8388608      # 8 MiB
tile_zoom = 22            # máximo 22
tile_extent = 4096        # máximo 65536
tile_buffer = 64          # ≤ tile_extent
export_features = 1000000
export_bytes = 268435456  # 256 MiB, incluye el cierre ]}
query_body_bytes = 131072 # 128 KiB
admin_body_bytes = 262144 # 256 KiB
page_bytes = 33554432     # 32 MiB, máximo 1 GiB

[auth]                    # opcional; si existe, las tres primeras son obligatorias
public_key_file = "/etc/atlas/jwt-public.pem"  # RSA 2048–8192 bits, SPKI o PKCS#1
issuer = "https://identity.example.test"
audience = "atlas"
leeway_seconds = 30       # 0..=300
roles_claim = "roles"     # ruta con puntos, p. ej. "realm_access.roles"

[cors]
allowed_origins = []      # vacío: sin CORS; ["*"] o hasta 100 "http(s)://host[:puerto]"
```

Salvo `pool_min`, `max_offset`, `tile_zoom` y `tile_buffer`, los límites deben ser mayores que 0.
Sin `[auth]` no hay administración. Atlas no emite tokens: verifica RS256 (`sub`, `iss`, `aud`,
`exp` y el arreglo de `roles_claim`, opcional) con la clave pública local, cargada una vez al arrancar.

## Publicar una capa

Token de ejemplo firmado con la clave privada **ficticia** de `rust/tests/fixtures/` (pública en
el repositorio; jamás usarla fuera de desarrollo). En producción el token lo emite el proveedor
de identidad con el rol `atlas-admin`.

```bash
b64() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
h=$(printf '{"alg":"RS256","typ":"JWT"}' | b64)
p=$(printf '{"sub":"dev","iss":"atlas-test","aud":"atlas","exp":%d,"roles":["atlas-admin"]}' \
  $(($(date +%s) + 3600)) | b64)
s=$(printf '%s.%s' "$h" "$p" | openssl dgst -sha256 -sign rust/tests/fixtures/jwt-test-private.pem | b64)
TOKEN="$h.$p.$s"

curl -fsS -X POST http://127.0.0.1:8081/api/v1/admin/datasets \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"id":"municipios","title":"Municipios",
       "source":{"schema":"geo","table":"municipios"},
       "properties":["clave","nombre","poblacion"],
       "access":{"public":true,"public_download":true}}'
```

La respuesta trae `ETag: "<revision>"`; PATCH (Merge Patch) y DELETE exigen `If-Match` con ese
valor. El token va siempre en la cabecera, nunca en la query string.

## Consultar

```bash
A=http://127.0.0.1:8081/api/v1
curl -fsS "$A/datasets"
curl -fsS "$A/datasets/municipios/items?bbox=-103,20,-102,22&properties=nombre&sort=poblacion:desc&limit=2"
curl -fsS -X POST "$A/datasets/municipios/query" -H 'Content-Type: application/json' \
  -d '{"filter":{"op":"gte","field":"poblacion","value":100000},"limit":10}'
curl -fsS -o z8.mvt "$A/datasets/municipios/tiles/8/55/112.mvt"
curl -fsS "$A/datasets/municipios/tilejson.json"
curl -fsS -OJ "$A/datasets/municipios/export?format=geojson"
```

Con una política privada se añade `-H "Authorization: Bearer $TOKEN"` con un token cuyos roles
estén en `view_roles`/`query_roles`/`download_roles`; `atlas-admin` no concede acceso a datos.
Una exportación incompleta (sin `]}` final) es una descarga fallida.

## Logs y eventos

Una línea JSON por evento en stderr; nunca cabeceras, cuerpos, tokens, SQL ni DSN.

| event | cuándo |
|---|---|
| `listening` | Puerto abierto, con `address` efectiva |
| `startup_failed` | Configuración, clave o DSN inválidos; sale con código no cero |
| `public_url_missing` | Al arrancar sin `public_url` (TileJSON usa rutas `/api/v1/...`) |
| `request` | Cada petición: `request_id`, `method`, `path` sin query, `status`, `duration_ms` y `code` si es error |
| `auth_failed` | Cada 401, con `request_id` y `reason` estable |
| `export_failed` | Exportación cortada tras enviar cabeceras, con `request_id` y `code` |
| `database_error` | 500 de la base, solo con `sqlstate` o `kind` |
| `shutdown_started` / `shutdown_forced` / `shutdown_complete` | SIGINT/SIGTERM: deja de aceptar, drena hasta `shutdown_seconds`, aborta lo pendiente y sale con 0 |

## Despliegue

La imagen `runtime` contiene solo el binario y corre como uid/gid 65532 (`ENTRYPOINT atlas`):

```bash
docker build --target runtime -t atlas-rust:experimental rust
docker run --rm --network host --env-file atlas-ddl.env -v "$PWD/atlas.toml:/etc/atlas.toml:ro" \
  atlas-rust:experimental migrate --config /etc/atlas.toml
docker run --rm --name atlas-rust --network host --env-file atlas-service.env \
  -v "$PWD/atlas.toml:/etc/atlas.toml:ro" -v "$PWD/public.pem:/etc/atlas/jwt-public.pem:ro" \
  atlas-rust:experimental serve --config /etc/atlas.toml --listen 127.0.0.1:8081
curl -fsS http://127.0.0.1:8081/api/v1/ready
docker stop --timeout 360 atlas-rust
```

Cada env-file define `ATLAS_DATABASE_URL`; nunca montar una clave privada. Migrar con un rol DDL
(propietario de `atlas_catalog`, CREATE sobre la base) y servir con un rol de servicio que solo
tenga USAGE en `atlas_catalog`, SELECT/INSERT/UPDATE/DELETE en `datasets`, SELECT en `migrations`
y USAGE/SELECT sobre las fuentes permitidas. Ambos necesitan USAGE sobre el esquema de la
extensión PostGIS (`SELECT extnamespace::regnamespace FROM pg_extension WHERE extname =
'postgis'`) y, si no es `public`, tenerlo en su `search_path`.

Detrás de un proxy, configurar `public_url` con el prefijo público: Atlas ignora `Forwarded` y
`X-Forwarded-*`. SIGTERM drena antes de cerrar: el plazo del supervisor debe cubrir
`shutdown_seconds`. Rutas, cabeceras, CORS y errores en [api.md](../docs/rust/api.md#transporte-y-coherencia);
evidencia en [acceptance.md](../docs/rust/acceptance.md).
