# Aceptación del MVP experimental

El motor vectorial Rust se valida contra PostGIS real y por HTTP; esta matriz no
certifica producción, conformidad OGC ni rendimiento superior a Python/GeoServer.
La API de referencia es [OpenAPI](openapi.yaml). No se migran consumidores.

| Requisito | Evidencia automatizada en `rust/tests/` |
|---|---|
| R01 publicación/persistencia | `acceptance`, `catalog`, `api_admin` |
| R02 tabla/vista/introspección | `acceptance`, `postgis_introspection` |
| R03 descubrimiento autorizado | `api_datasets` |
| R04 selección compartida | `postgis_selection`, `postgis_features`, `api_export`, `api_tiles` |
| R05 predicados/tipos | `query_predicate`, `postgis_predicates` |
| R06 espacio/topología | `postgis_spatial`, `postgis_selection` |
| R07 filtro fijo AND usuario | `postgis_selection`, `api_export`, `api_features` |
| R08 páginas | `postgis_features`, `api_features` |
| R09 GeoJSON/precisión | `postgis_features`, `exports_geojson` |
| R10 MVT/TileJSON | `postgis_mvt`, `api_tiles`, `acceptance` |
| R11 export/cursor | `exports_geojson`, `api_export`, `admission` |
| R12 JWT externo RS256 | `auth_jwt`, `auth_http` |
| R13 políticas | `auth_policy`, `api_admin`, `api_export`, `acceptance` |
| R14 privado por defecto | `domain`, `auth_policy`, `api_datasets` |
| R15 presupuestos/cancelación | `admission`, `postgis_connection`, `api_features`, `api_export` |
| R16 operación/shutdown | `main`, `acceptance`; prueba de contenedor descrita en `rust/README.md` |
| R17 convivencia Python | job Python de CI; no cambia `src/atlas` ni consumidores |

`acceptance` ejecuta el binario `atlas migrate/serve`, publica una tabla pública y una
vista privada mediante administración HTTP, reinicia el proceso y comprueba detalle,
items, export, TileJSON y MVT. Valida estado, selección e id; las pruebas especializadas
cubren cabeceras, errores y tipos. Las rutas y respuestas se contrastan con OpenAPI,
sin afirmar validación exhaustiva de todas las combinaciones del esquema.

```bash
cd rust
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
ATLAS_TEST_DATABASE_URL=postgresql://atlas:atlas@localhost:5432/atlas_rust_test cargo test --locked
```

La BD debe ser descartable: las pruebas crean/eliminan esquemas y publicaciones de prueba.
La suite no utiliza datos de producción ni modifica fuentes Python. OIDC/JWKS, formatos
adicionales, comparación de rendimiento y piloto de consumidor quedan en sus issues.
