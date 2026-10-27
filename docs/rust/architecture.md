# Arquitectura objetivo y reutilización

Un único paquete Cargo en `rust/`, con binario `atlas` y módulos internos reutilizables.
No comprometer un SDK estable ni dividir en crates/microservicios antes de necesitarlo.

```text
API Axum ── identidad JWT ── servicios de publicación/consulta/tiles/export
                                      │
                       modelos de dataset, Query y políticas
                                      │
                   catálogo persistente + adaptador PostGIS SQLx
                                      │
                    PostgreSQL/PostGIS (una BD configurada)
```

## Fronteras

- domain: Dataset/Query/Predicate/AccessPolicy y límites; no conoce HTTP ni SQLx.
- postgis: pool, introspección, compilación de predicados, lectura por cursor y MVT.
- catalog: persistencia en atlas_catalog y revisión de publicación; no es un ORM de datos fuente.
- services: autorizar y combinar restricciones antes de I/O; comparten Query, no paginación.
- api/auth: validación de transporte, JWT→Principal y errores; no construyen SQL.
- main/config: configuración, migraciones explícitas, wiring y shutdown. TLS/rate limiting en proxy.

Stack elegido: Rust estable fijado al crear Cargo, Tokio, Axum, SQLx Postgres, Serde/serde_json,
tracing, jsonwebtoken, TOML y Tower HTTP. Solo features necesarias; Cargo.lock versionado.
No se instala nada en esta entrega. OpenAPI es contrato versionado y se prueba contra el servidor.
Adaptador SQL: push_bind para valores; identificadores de introspección/whitelist con quoting
de identificadores, nunca push de texto arbitrario del cliente. No aceptar SQL como configuración
de dataset: publicar una vista cuando se necesite una consulta preparada por el administrador.

## Qué se porta desde Python

| Pieza | Activo reutilizable |
|---|---|
| postgis/introspection.py | SQL pg_catalog/geometry_columns, reglas de tablas/vistas y SRID |
| core/predicates.py y postgis/predicates.py | AST, tipos, NULL/NOT, presupuestos y prefiltrado indexable |
| postgis/queries.py y values.py | Selección/orden con whitelist, parámetros y conversión de tipos |
| postgis/mvt.py y tiles/ | ST_AsMVTGeom/ST_AsMVT, buffers/bordes/IDs y presupuestos previos al payload |
| connection.py y exports/geojson.py | Cursor por lotes, cancelación, cierre y semántica de salida |
| layers/ | Metadatos y operaciones de permiso; registry en memoria pasa a catálogo persistente |
| tests/ y benchmarks/vector_* | Fixtures, ataques, oráculos de cobertura/selección y métricas |

Se reutiliza conocimiento/SQL/pruebas, no se enlazan clases Python desde Rust.
FastAPI, Pydantic, psycopg, Shapely, Pillow, XML OGC y render de imágenes quedan en el legado.
MVT se produce en PostGIS; Rust no decodifica geometrías para volver a codificarlas.

## Catálogo y consistencia

Migración inicial crea atlas_catalog.datasets: id, configuración validada JSONB, metadatos
inspeccionados JSONB, revision bigint y timestamps. id único; PATCH/DELETE con If-Match de la
revision para evitar actualizaciones perdidas (412). Config/metadata siempre validados al leer.
Un repositorio concreto SQLx; sin interfaz genérica para otros backends.
No caché de catálogo en el MVP: resolver publicación vigente por petición evita invalidación
entre instancias. Actualizar publicación no revoca un stream ya autorizado; aplica a solicitudes
nuevas. Deshabilitar no toca las tablas; cambios de datos se ven en nuevas transacciones.
Sin caché de resultados; respuestas de datos privadas y dinámicas con Cache-Control: no-store.
OIDC añadirá un verificador real de issuer/JWKS en fase 3; no crear un framework de proveedores.

## Referencias

- [Axum](https://docs.rs/axum/latest/axum/) y [SQLx QueryBuilder](https://docs.rs/sqlx/latest/sqlx/struct.QueryBuilder.html).
- [ST_AsMVT](https://postgis.net/docs/ST_AsMVT.html) y [ST_AsMVTGeom](https://postgis.net/docs/ST_AsMVTGeom.html).
- [JWT](https://docs.rs/jsonwebtoken/latest/jsonwebtoken/); no implementar criptografía propia.
