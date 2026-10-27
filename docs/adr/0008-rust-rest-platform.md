# ADR 0008 — Atlas como motor Rust y API REST

Estado: dirección aprobada por el responsable del proyecto; implementación pendiente.
Fecha: 2026-10-27. Seguimiento: [#203](https://github.com/DatzinDev/Atlas/issues/203).

## Contexto

Atlas Python/FastAPI publica PostGIS mediante WMS/WFS y REST. Se busca una plataforma ligera,
modular y reutilizable centrada en formatos actuales y consumidores HTTP. La nueva dirección
reemplaza para Rust los límites de librería embebida y enfoque OGC de ADRs 0001–0003 y #101;
no cambia retroactivamente los contratos ni la disponibilidad de Python.

## Decisión

Rust + Tokio/Axum/SQLx en un proceso, PostGIS como motor espacial. REST propio versionado,
catálogo persistente administrable, consultas y exportación GeoJSON y MVT en el MVP.
JWT emitidos por la aplicación integradora; Atlas valida identidad y autoriza por publicación.
Sin login ni usuarios propios. OIDC posterior. GeoParquet/FlatGeobuf y PMTiles posteriores.
Python y Rust coexisten hasta migrar consumidores con retorno probado.

Se aprueban para el nuevo paquete las dependencias de arquitectura documentadas en
[Atlas Rust](../rust/architecture.md). La stdlib no ofrece HTTP async, PostgreSQL, JWT ni
serialización completos; no reimplementar protocolos o criptografía para evitar dependencias.
No se amplían las dependencias del runtime Python. Cada formato posterior requiere su ADR.

## Alternativas

- Optimizar solo Python: menor coste, pero no cumple la transición completa de producto solicitada.
- Go: válido para un servidor; se elige Rust por la dirección explícita aprobada y el control de recursos.
- Extensiones Rust con bindings Python: preservan embebido, pero no son el motor/API independiente elegido.
- Traducir OGC/renderer completo: aumenta alcance sin contribuir al MVP moderno.
- Microservicios o plugins: añaden operación y contratos sin implementaciones que los requieran.

## Consecuencias

Reutilizar SQL, invariantes y fixtures; reescribir integración y persistencia de catálogo.
El rendimiento no se deduce del lenguaje: conservar evidencia Python y comparar cargas válidas
contra Rust y GeoServer. MVT no equivale a una descarga íntegra ni a un render WMS.
No retirar Python, fusionar PRs previos ni cerrar pendientes por aprobar este ADR.
Referencias y criterios: [SDD](../rust/spec.md), [rendimiento](../rust/benchmarks.md).
