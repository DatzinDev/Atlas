# Plan de migración por fases

Estado: planificación publicada para revisión; sin motor Rust ni nuevas mediciones ejecutadas.
Épica [#203](https://github.com/DatzinDev/Atlas/issues/203); [tareas con dependencias](tasks.md).

| Milestone | Entrega y condición de cierre |
|---|---|
| [Rust 0 — Especificación y línea base](https://github.com/DatzinDev/Atlas/milestone/14) | SDD/ADR/OpenAPI, archivo de métricas Python existentes y herramienta HTTP reutilizable; sin nuevas mediciones ni GeoServer. |
| [Rust 1 — Publicación y acceso](https://github.com/DatzinDev/Atlas/milestone/15) | Paquete/CI, pool, modelos, introspección, catálogo, JWT/políticas y filtros seguros para registrar/publicar tabla y vista. |
| [Rust 2 — MVP vectorial](https://github.com/DatzinDev/Atlas/milestone/16) | GeoJSON, selección/orden/páginas, MVT/TileJSON y descarga; recursos acotados, contenedor y aceptación integrada. |
| [Rust 3 — Validación y adopción](https://github.com/DatzinDev/Atlas/milestone/17) | Comparación de los tres motores, OIDC y consumidor piloto especificado/migrado con retorno probado. |
| [Rust 4 — Formatos modernos](https://github.com/DatzinDev/Atlas/milestone/18) | ADR de interoperabilidad, GeoParquet y después FlatGeobuf, límites y lectores independientes. |
| [Rust 5 — Publicaciones estáticas](https://github.com/DatzinDev/Atlas/milestone/19) | ADR y CLI PMTiles por dataset/extensión/zooms, archivo versionado verificable y acceso conforme a política. |

Milestones ordenan entregas, no fechas ni versiones comprometidas. #210 cierra Python con
métricas conservadas y herramienta #208 revisadas/integradas; no exige una nueva línea base.
GeoServer #209 queda en Rust 3 y no bloquea #211. Ninguna tarea se cierra por existir este plan.
Las dependencias, no el número de issue, fijan el orden. Parsing/SQL/selección espacial segura
pertenecen a Rust 1 porque el API administrativo valida filtros fijos antes de publicar.

## Secuencia operativa

1. Integrar los cuatro PRs documentales revisados mediante gh stack; conservar originales Python.
2. Integrar #208 y #210: herramienta HTTP, métricas existentes y sus limitaciones. Cerrar etapa Python.
3. Implementar Rust 1 por contratos; probar migración explícita y persistencia del catálogo.
4. Completar Rust 2 y cruzar todos los R01–R17 contra tests reales; empaquetar experimental.
5. Medir Rust y una referencia Python HTTP equivalente; incorporar GeoServer después (#209),
   resolver regresiones observadas en issues independientes
   y ensayar un consumidor; OIDC no cambia el modelo de políticas.
6. Especificar formatos adicionales antes de dependencias; PMTiles usa publicación estática.

Sin Redis, Kubernetes, colas, bindings Python, framework de plugins, UI ni motor espacial propio.
GeoParquet usa 1.1/WKB; FlatGeobuf indexado y PMTiles se completan antes de publicarse.
Para algoritmos complejos en el MVP se registran vistas PostGIS, sin SQL arbitrario de clientes.

## Convivencia y retorno

Python mantiene API y paquetes; Rust usa prefijo /api/v1 y una configuración de despliegue aparte.
No reescribir URLs de consumidores automáticamente. El piloto primero inventaría las operaciones
que realmente usa y define aceptación, autorización y vuelta a los endpoints Python/GeoServer.
Las tablas fuente son compartibles solo para lectura; el catálogo nuevo tiene esquema separado.
Las migraciones no borran ni renombran relaciones publicadas.
Cambiar el enrutamiento del consumidor permite retorno sin migrar los datos fuente.
No se considera reemplazo completo de GeoServer mientras existan dependencias de raster/OGC.
Retirar Python/OGC o cambiar releases de PyPI requiere una decisión posterior explícita.

## Backlog anterior

- #100 se mantiene como índice histórico Python, enlazado con #203.
- #193–#196 y PRs #197–#200 permanecen en revisión independiente: no fusionarlos ni cerrarlos aquí.
- Ráster Python #111/#114/#115/#117/#119/#120/#122/#123/#124/#129/#132/#135/#136/#137/#139
  y optimización #202 quedan deferred y track:python; no son prerrequisitos Rust.
- Conservar contratos y evidencia ya entregados. Una idea transferible requiere su propia tarea Rust.

## Trabajo por issue

Leer spec/architecture/api y el requisito enlazado; implementar solo el contrato. Un módulo/test,
validación de límites/permisos y PostGIS real cuando corresponda. Los ADRs autorizan dependencias
por capacidad, no una actualización general de paquetes. Mantener las verificaciones Python.
PRs pequeños (<300 líneas de diff); si exceden, descomponer. Una rama/issue, Conventional Commits
en inglés de una línea sin trailers, PR en español contra develop y Closes #N.
Stacks solo gh stack init/add/submit/sync/view, nunca cadenas manuales con gh pr --base.
Revisión antes de merge; main solo recibe releases desde develop. Sin commits en ramas base.
SDD/contrato y tests cambian en el mismo PR. No cerrar tareas por simples artifacts/HTTP200.
