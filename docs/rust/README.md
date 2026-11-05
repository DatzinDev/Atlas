# Atlas Rust — especificación y ejecución

Estado: **MVP vectorial Rust experimental implementado**, pendiente de revisión e integración de esta ronda. Seguimiento: [#203](https://github.com/DatzinDev/Atlas/issues/203).
La versión Python conserva sus contratos y su publicación actuales durante la transición.

| Documento | Autoridad |
|---|---|
| [Aceptación](acceptance.md) | Matriz R01–R17, pruebas y límites de la evidencia |
| [Especificación](spec.md) | Requisitos y aceptación del MVP |
| [Arquitectura](architecture.md) | Responsabilidades, reutilización y fronteras |
| [Contrato REST](api.md) y [OpenAPI](openapi.yaml) | Contrato HTTP del MVP experimental |
| [Métricas Python](benchmarks.md) | Archivo consolidado del cierre Python y límites de comparación |
| [Benchmark reutilizable](../../benchmarks/COMPARE.md) | Medir por HTTP al disponer del servidor Rust |
| [Plan](plan.md) y [tareas](tasks.md) | Etapas, dependencias y enlaces a GitHub |
| [Verificación documental](verification.md) | Comprobaciones realizadas y pruebas pendientes |
| [ADR 0008](../adr/0008-rust-rest-platform.md) | Decisión de producto y tecnología |

SDD significa desarrollo guiado por especificaciones: requisito → issue → prueba → PR → evidencia.
Un issue `ai-ready` tiene contrato y aceptación; `blocked` impide iniciarlo hasta integrar sus
dependencias. Las capacidades futuras `needs-spec` requieren especificación antes de código.
Crear documentación, un issue o un contenedor no demuestra implementación ni despliegue.

El MVP publica tablas/vistas PostGIS, sirve consultas GeoJSON y tiles MVT y descarga GeoJSON.
Incluye catálogo persistente administrable y acceso público/privado con JWT externos.
GeoParquet, FlatGeobuf, OIDC y snapshots PMTiles tienen etapas posteriores.
La licencia MIT permanece vigente. No se reproduce la administración ni todos los protocolos de GeoServer.
