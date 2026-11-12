# ADR 0010 — Verificación OIDC con JWKS del operador

Estado: propuesto; requiere aprobación del responsable del proyecto antes de implementar.
Fecha: 2026-11-12. Seguimiento: [#232](https://github.com/DatzinDev/Atlas/issues/232).

## Contexto

Atlas verifica hoy JWT RS256 con una clave PEM local (#216). Un proveedor OIDC publica sus
claves en un JWKS y las rota por `kid`; copiar el PEM a mano rompe la verificación en cada
rotación. R12–R14 no cambian: Atlas sigue sin login, usuarios, emisión ni refresh de tokens,
y el `Principal` y las políticas son los mismos. La stdlib y el árbol actual no traen un
cliente HTTPS: Axum aporta hyper 1, hyper-util, http-body-util y SQLx aporta rustls 0.23 con
ring y webpki-roots, pero falta el conector TLS para el cliente.

## Decisión

Agregar `hyper-rustls` 0.27 sin features por defecto, con `http1`, `tls12`, `ring` y
`webpki-tokio`, y activar en `hyper-util` las features `client-legacy`, `http1` y `tokio`.
Solo entran `hyper-rustls` y `tokio-rustls`; ring y las raíces webpki ya están en el árbol.
`jsonwebtoken` ya interpreta JWK y JWKS, así que no se agrega parser propio.

Contrato del verificador, en `rust/src/auth/oidc.rs`:

- `[auth]` admite exactamente uno de `public_key_file` (modo actual, sin cambios) o
  `jwks_url`. Con `jwks_url` también son obligatorios `issuer`, `audience` y `roles_claim`.
- `jwks_url` es HTTPS; HTTP solo se acepta hacia loopback para pruebas. Sin discovery
  (`.well-known/openid-configuration`): el operador da la URL. Nada del token (`iss`, `jku`,
  `x5u`, `kid`) elige URL ni clave de confianza; `kid` solo busca dentro del JWKS ya cargado.
- Algoritmo permitido: RS256. Se ignoran las JWK que no sean RSA de firma (`use` `sig` o
  ausente, `alg` RS256 o ausente). Validación temporal igual que el modo PEM.
- `roles_claim` es una ruta con puntos (`roles`, `realm_access.roles`); el valor debe ser un
  arreglo de strings. Ausente equivale a lista vacía; otro tipo es 401.
- Caché en memoria por proceso, solo de claves públicas: carga al arrancar, renovación al
  ver un `kid` desconocido con una sola descarga en curso y como máximo una cada
  `jwks_min_refresh` (60 s), y renovación por antigüedad tras `jwks_max_age` (1 h).
  Límites: 64 KiB por respuesta, 32 claves, 5 s por descarga, sin redirecciones.
- Fallo cerrado: si la descarga falla, se conservan las claves vigentes hasta
  `jwks_max_stale` (24 h); después, todo token es 401. Un `kid` desconocido sin renovación
  posible es 401. El arranque falla si la primera carga no da al menos una clave válida.
- La caché no guarda tokens, claims ni respuestas; los datos privados siguen con
  `Cache-Control: no-store`. Logs sin tokens ni cuerpos del JWKS.

## Alternativas

- `reqwest`: API más cómoda, pero suma más crates y otra capa de configuración TLS para una
  sola petición GET.
- Implementar TLS o HTTP sobre `tokio::net`: reimplementa protocolos; descartado por ADR 0008.
- Discovery OIDC: una petición y un punto de confianza más; el operador ya conoce la URL.
- Renovación periódica en segundo plano: tarea extra que mantener viva; la renovación bajo
  demanda con límites cubre la rotación.
- Varios proveedores a la vez: no lo pide el contrato; sería un framework de proveedores.

## Consecuencias

El modo PEM se conserva para despliegues sin proveedor. `verify` pasa a ser asíncrono en
modo JWKS y el middleware lo espera; el resto del pipeline no cambia. Las pruebas levantan un
JWKS controlado en loopback para cubrir rotación, `kid` desconocido, caída, timeout, otro
issuer o algoritmo y expiración. La entrega va en un stack de PRs: este ADR, el verificador y
su caché, y la integración con configuración, middleware y documentación del contrato.
