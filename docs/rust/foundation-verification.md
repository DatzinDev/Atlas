# Verificación del bloque #212–#215

Implementación sobre develop e9ee488; Rust 1.99.1 y Cargo.lock versionados.
El responsable autorizó preparar el bloque como stack sobre dependencias aún
sin merge. Cada capa manual tiene menos de 300 líneas; el PR separado de SQLx
incluye la excepción aprobada para Cargo.lock generado.

## Reproducción local

BD exclusivamente descartable, sin volúmenes ni datos reales:

```bash
docker run -d --name atlas-rust-block-test \
  -e POSTGRES_USER=atlas -e POSTGRES_PASSWORD=atlas_test_only \
  -e POSTGRES_DB=atlas_rust_test -p 127.0.0.1:55439:5432 postgis/postgis:16-3.4
docker exec atlas-rust-block-test pg_isready -U atlas -d atlas_rust_test
cd rust
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
ATLAS_TEST_DATABASE_URL=postgresql://atlas:atlas_test_only@127.0.0.1:55439/atlas_rust_test cargo test --locked
docker rm -f atlas-rust-block-test
```

Esperar pg_isready antes del test. CI usa el mismo comando Cargo con un servicio
PostGIS dedicado; no omite integración silenciosamente cuando falta DSN.
Los ejemplos contienen credenciales ficticias de la BD descartable.

## Cobertura y límites

- R01: CRUD persistente tras reabrir pool, CAS concurrente, rollback ante error,
  rechazo de metadatos corruptos y borrado sin eliminar fuente.
- R02/R04/R08: tabla/vista/materializada, nombres con comillas, geometría explícita
  o única, vacía tipada/genérica, IDs nulos/duplicados y PK compuesta; propiedades
  explícitas, sin geometrías ni tipos incompatibles.
- R12–R15: modelos cerrados, privacidad por defecto, roles como datos, límites,
  bbox finito y numeric exacto. JWT y autorización operativa siguen en #216/#217;
  AST/validación semántica de filtros en #220, antes de API administrativa (#218).
- R15/R16: pool saturado, timeout SQL, error/rollback, cancelación con trabajo
  restante acotado por statement_timeout, desconexión y reutilización. Shutdown
  SIGINT/SIGTERM real y readiness con/sin BD; configuración inválida sin secretos.
- DDL explícito/repetible, separado del servicio; CRUD con rol sin permisos DDL.

No es una medición de rendimiento, un despliegue ni aceptación del MVP completo.
La cancelación no promete interrupción SQL inmediata; SQLx devuelve la conexión
tras limpiar la transacción o cerrarla y PostgreSQL acota el statement restante.
La inferencia genérica toma una muestra, no demuestra homogeneidad de todas las
filas. Cambios de esquema externos posteriores se tratarán en las consultas.
