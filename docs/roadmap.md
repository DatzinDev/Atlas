# Roadmap

Las fases 0–4 del MVP están cerradas (43 issues). Las etapas nuevas se siguen en
[#100](https://github.com/DatzinDev/Atlas/issues/100); no equivalen a versiones ni fechas.

## v0.1.0 — MVP

El criterio de éxito es el §42 de `requirements.md`: registrar una tabla PostGIS, verla, consultarla,
filtrarla, descargarla y exportarla como PNG, sin levantar GeoServer.

| Milestone | Contenido | Resultado |
|---|---|---|
| **Fase 0 — Fundaciones** | pyproject, docker-compose con PostGIS, CI, fixtures, spike | El stack elegido está validado y hay dónde correr tests |
| **Fase 1 — Núcleo** | config, excepciones, pool, introspección, `Layer`, `LayerRegistry`, CRS, permisos, logging, `Atlas.router`, `GET /layers` | `geo.register_layer("geo.municipios")` funciona |
| **Fase 2 — WFS** | GetCapabilities, DescribeFeatureType, GetFeature, bbox, paginación, filtros, selección de propiedades | El geoportal puede consultar datos vectoriales |
| **Fase 3 — WMS** | GetCapabilities, GetMap, GetFeatureInfo, estilos de polígono/línea/punto, PNG/JPEG, `POST /maps/export` | El geoportal puede visualizar y exportar mapas |
| **Fase 4 — Exportación** | GeoJSON y CSV en streaming, `GET /layers/{layer}/download` con filtros, GeoPackage, Shapefile ZIP | El usuario puede bajarse los datos |

## Etapas activas — contratos antes de implementación

| Etapa / milestone | Issues | Cierre esperado |
|---|---:|---|
| [1. Contratos y corrección](https://github.com/DatzinDev/Atlas/milestone/9) | 7 | Contratos y correcciones verificados |
| [2. Consultas compartidas](https://github.com/DatzinDev/Atlas/milestone/7) | 12 | Filtros y permisos consistentes |
| [3. MVT nativo](https://github.com/DatzinDev/Atlas/milestone/10) | 5 | Tiles autorizados y acotados |
| [4. Ráster: fuentes y procesamiento](https://github.com/DatzinDev/Atlas/milestone/8) | 8 | Ventanas, estilos y recursos verificados |
| [5. Ráster: integración y API](https://github.com/DatzinDev/Atlas/milestone/11) | 6 | API y extra aislados |
| [6. Validación de rendimiento](https://github.com/DatzinDev/Atlas/milestone/12) | 2 | Benchmarks reproducibles |

Las dependencias de cada issue permiten trabajo paralelo. Tests y límites se verifican
por tarea, antes del benchmark final. #101 fija alcance; #105–#107 definen los contratos.
Nada de este lote se presenta como implementado por existir documentación.

Administración queda [futura](https://github.com/DatzinDev/Atlas/milestone/6), sin programación;
no bloquea las etapas. MVT usa PostGIS; ráster será GeoTIFF/COG local mediante extra opcional.
WCS/WMTS completos, fuentes HTTP/S3 y PostGIS Raster se difieren. El host conserva
las políticas de producto, autenticación y despliegue; no se añaden servicios externos.

## Fuera de alcance permanente

`WFS-T`, `LockFeature`, stored queries, SLD completo, GML completo, WPS, CSW, Oracle/MySQL/ArcSDE,
Shapefile o GeoPackage **como datasource**, edición remota, motor propio de reproyección, motor
propio de rendering, procesamiento raster propio.

Antes de aceptar cualquiera de estos, se aplica el principio rector del §45: *¿esta capacidad es
necesaria para publicar, visualizar, consultar o descargar información geoespacial desde una
aplicación web?*

Contrato de consultas en revisión: [ADR 0005](adr/0005-shared-query-predicates.md).
Su publicación no implementa los issues dependientes de consultas.
