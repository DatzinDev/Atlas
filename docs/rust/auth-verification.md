# Verificación de identidad y permisos (#216–#217)

## Alcance

RS256 con clave pública local o JWKS OIDC (#232); sin login, emisión de tokens ni claves reales.
Transporte Bearer integrado en `serve`; políticas concretas sin I/O disponibles para
los futuros handlers. Los endpoints administrativos y de datos siguen pendientes.
Los PRs se preparan en stack con autorización del responsable para avanzar #217
sobre #216 sin integrar. No se realiza merge ni despliegue.

## Reproducir

Desde `rust/`, con Rust fijado por el repositorio:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --test auth_jwt --test auth_jwks --test config --test auth_http --test auth_policy
```

HTTP abre sockets locales; no usa mocks de transporte. Para la regresión completa,
preparar PostGIS descartable y `ATLAS_TEST_DATABASE_URL` como en
[fundamentos](foundation-verification.md); ejecutar `cargo test --locked`.

## Evidencia cubierta

- R12: firma alterada, none/HS256/RS512, issuer/audience incorrectos o ausentes,
  sub/exp faltantes, expiración, nbf futuro, tipos inválidos y claims duplicados.
- PEM público exclusivamente; jku/kid no resuelven confianza ni hacen red.
  Claims adicionales permitidos; roles ausentes equivalen a lista vacía.
- Llave RSA fuera de 2048..=8192 bits, exponente par o < 3, o DER truncado:
  `Verifier::new` falla y `serve` sale con `startup_failed` antes del listener.
- `auth.leeway_seconds` (0..=300, por defecto 30) aplica a `exp` y `nbf`;
  `exp` de hace 20 s es válido por defecto e inválido con 0; 301 invalida la config.
- R14: anónimo sin cabecera; cualquier token falla si no hay verificador configurado.
  Inválido devuelve 401 incluso en ruta pública, antes de ejecutar el handler.
  Cabeceras repetidas/malformadas fallan; query string no autentica.
- A5: cada 401 registra `{"event":"auth_failed","request_id":"…","reason":"<r>"}` con
  `r` ∈ `multiple_headers`, `malformed_header`, `unsupported_scheme`, `auth_disabled`,
  `token_too_large`, `malformed_token`, `invalid_signature`, `invalid_algorithm`,
  `expired`, `not_yet_valid`, `invalid_issuer`, `invalid_audience`, `invalid_claims` y, en
  modo JWKS, `missing_kid`, `unknown_kid`, `jwks_unavailable` y `jwks_expired`.
  `Verifier::verify` y `auth::http::principal` devuelven ese motivo (`&'static str`,
  derivado de `ErrorKind`). La respuesta no cambia (401 `invalid_token`, `Bearer`) y el
  log nunca incluye token, cabecera, `sub` ni roles.
- JWKS (#232, `auth_jwks`): proveedor en loopback; solo JWK RSA de firma, con `kid` y fuertes;
  fallo cerrado al arrancar (5xx, redirección, timeout, > 64 KiB, sin claves); rotación por
  `kid` con una sola descarga; caída tolerada hasta el vencimiento; leeway y `roles_claim`
  anidado. `roles_claim` aplica igual al modo PEM y rechaza llaves repetidas en la ruta.
- Arranque real con TOML/PEM local, rechazo de clave inválida antes del listener,
  health/readiness y cierre limpio; respuestas/logs no incluyen el token de prueba.
- R13: 256 combinaciones de public/public_download/enabled, identidad y roles;
  view previo a query/download, 404 invisible/deshabilitado y 403 operación faltante.
  atlas-admin no implica acceso a datos. Filtro de visibilidad previo a paginación
  demostrado en memoria; filtros OR/NOT no conceden permisos.

## Límites de la evidencia

Las políticas aún no protegen endpoints geoespaciales: esos endpoints no existen;
#218–#219 y siguientes deben invocarlas antes del acceso a datos. No se demuestra
todavía paginación HTTP de catálogo, SQL de filtros ni revocación de streams.
Rotación de PEM requiere reinicio; reloj del host sincronizado dentro de `leeway_seconds`.
Sin pruebas de carga, auditoría criptográfica propia ni validación en producción.
