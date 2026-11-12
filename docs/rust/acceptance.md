# Aceptación del MVP experimental

El motor vectorial Rust se valida contra PostGIS real y por HTTP; esta matriz no
certifica producción, conformidad OGC ni rendimiento superior a Python/GeoServer.
La API de referencia es [OpenAPI](openapi.yaml). No se migran consumidores.

Cada fila cita tests concretos como `archivo::función` (`rust/tests/archivo.rs`) y cada
test citado lleva el comentario `// R0x` sobre su atributo `#[test]`/`#[tokio::test]`.
`acceptance::requirements_are_traced` lee esta matriz y falla si falta un R-ID o si una
función citada no existe.

| Requisito | Evidencia automatizada en `rust/tests/` |
|---|---|
| R01 publicación/persistencia | `acceptance::experimental_mvp_survives_restart`, `catalog::persistent_publications_compare_and_swap_and_source_survives`, `api_admin::admin_routes_over_real_http` |
| R02 tabla/vista/introspección | `postgis_introspection::real_relations_geometry_identifiers_and_properties`, `postgis_introspection::geometry_without_typmod_requires_single_type_and_srid` |
| R03 descubrimiento autorizado | `api_datasets::discovery_lists_only_authorized_datasets_over_real_http` |
| R04 selección compartida | `domain::supported_types_have_one_source_of_truth`, `postgis_features::feature_pages_against_real_postgis`, `api_export::downloads_over_http`, `api_tiles::vector_tiles_over_real_http` |
| R05 predicados/tipos | `query_predicate::parses_attribute_predicates_and_preserves_numbers`, `query_predicate::parses_logical_groups`, `postgis_predicates::attribute_filters_against_real_postgis` |
| R06 espacio/topología | `query_predicate::validates_selection_geometry_structure`, `postgis_spatial::topology_transform_and_indexable_predicates`, `postgis_selection::each_geometry_stays_bound_to_its_own_node` |
| R07 filtro fijo AND usuario | `postgis_selection::fixed_user_and_bbox_cannot_widen_each_other` |
| R08 páginas | `postgis_features::feature_pages_against_real_postgis`, `api_features::items_and_query_over_real_http` |
| R09 GeoJSON/precisión | `postgis_features::feature_pages_against_real_postgis`, `api_features::items_and_query_over_real_http` |
| R10 MVT/TileJSON | `postgis_mvt::native_mvt_against_real_postgis`, `api_tiles::vector_tiles_over_real_http` |
| R11 export/cursor | `exports_geojson::cursor_budgets_snapshot_and_cleanup`, `api_export::downloads_over_http`, `api_export::cuts_oversized_downloads_mid_stream` |
| R12 JWT externo RS256 | `auth_jwt::valid_identity_and_local_trust_only`, `auth_jwt::invalid_claims_fail_closed`, `auth_http::real_http_authenticates_before_handlers` |
| R13 políticas | `auth_policy::permission_matrix_is_closed_and_operations_require_visibility`, `api_admin::admin_routes_over_real_http` |
| R14 privado por defecto | `domain::closed_models_and_private_defaults`, `auth_http::real_http_authenticates_before_handlers` |
| R15 presupuestos/cancelación | `admission::cancelled_callers_keep_slots_until_workers_finish`, `admission::stalled_exports_cannot_starve_items_and_tiles`, `postgis_connection::timeout_saturation_rollback_cancel_and_close` |
| R16 operación/shutdown | `main::http_startup_and_shutdown`, `main::shutdown_deadline_aborts_stalled_work`, `main::configuration_errors_do_not_echo_input`; prueba de contenedor descrita en `rust/README.md` |
| R17 convivencia Python | Excepción explícita: sin test Rust; evidencia en el job Python de CI, que no cambia `src/atlas` ni consumidores |

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

## Evidencia de esta ronda

- Toolchain 1.99.1: `fmt --check`, Clippy con warnings como errores y suite Rust completa
  contra PostGIS 16/3.4 descartable; aceptación del binario y cancelación incluidas.
- Python: Ruff, formato, mypy y 768 tests pasan en entorno aislado (Python 3.13).
  El job de CI conserva su ejecución en Python 3.11.
- Imagen construida desde cero con Cargo offline en el paso release; `migrate` exitoso,
  health/readiness 200, uid 65532, sin Cargo/fuentes y SIGTERM con `shutdown_complete`.
- Verificación local únicamente: sin despliegue, publicación de imagen ni piloto de consumidor.
