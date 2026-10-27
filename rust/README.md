# Atlas Rust: arranque inicial (#211)

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
de Axum. Logs JSON mínimos en stderr, sin registrar solicitudes ni cabeceras.
El cierre del servidor espera las conexiones prestadas y cierra el pool.

## Configuración y dominio (#212)

`serve --config archivo.toml` admite `listen`, `database_url`, `allowed_schemas`
y tabla `[limits]` con los defaults de spec.md. No acepta claves desconocidas.
`ATLAS_DATABASE_URL` y `ATLAS_LISTEN` prevalecen sobre TOML; `--listen` prevalece
sobre el entorno. DSN nunca pertenece a un Dataset ni se imprime al fallar.
La configuración se valida antes del bind/pool. No derive Debug/Serialize en Config.
El DSN es opcional solo para health; para operar datos es obligatorio.

Domain no importa HTTP/SQL: Dataset/Source/AccessPolicy/Principal, Query y Error.
Después de deserializar se llama `validate` con schemas/límites del servidor;
la introspección posterior verifica propiedades y tipos reales. Los filtros se
conservan como RawValue sin redondear numeric: su parser acotado y validación
semántica pertenecen a #220, antes de exponer publicaciones o consultas por HTTP.

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

## Migración explícita (#215)

`atlas migrate --config ddl.toml` crea `atlas_catalog.datasets` en una transacción
con bloqueo de migración. Repetirla es seguro; `serve` nunca ejecuta DDL.
Usar un rol DDL separado: propietario del esquema/tabla. La cuenta de servicio
recibe únicamente USAGE en atlas_catalog y SELECT/INSERT/UPDATE/DELETE en datasets,
además de USAGE/SELECT en los esquemas/relaciones fuente permitidos. Nunca concederle
CREATE ni propiedad de esos objetos. Las pruebas demuestran el rechazo de DDL con
un rol limitado; no modifican roles ni fuentes de ningún despliegue existente.

`Catalog` valida configuración y metadatos al leer, vuelve a inspeccionar al crear
o reemplazar y no usa caché. Update/delete comparan revisión en el mismo statement;
una revisión obsoleta devuelve RevisionMismatch. Borrar retira solo publicación,
nunca datos fuente. El repositorio recibe configuración completa: Merge Patch y
ETag de transporte se incorporan en #218; el filtro opaco se valida en #220 antes
de habilitar publicación por HTTP. No hay DDL automático en operaciones CRUD.
