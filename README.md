# Atlas

Librería Python para FastAPI que publica tablas y vistas de **PostGIS** como capas geográficas
vía **WMS** y **WFS**, con API de consulta, filtrado, descarga y exportación de mapas.

Sin GeoServer. Sin proceso aparte. Es una librería que vive dentro de tu aplicación.

> **Estado:** v0.1.0 — MVP disponible como release en GitHub. Ver [roadmap](docs/roadmap.md).

## Instalación

```bash
pip install atlas-geo              # núcleo: WMS, WFS, API, GeoJSON, CSV
pip install atlas-geo[export]      # + GeoPackage y Shapefile
```

Requiere Python 3.11+ y PostgreSQL 14+ con PostGIS 3.

## Uso

```python
from fastapi import FastAPI
from atlas import Atlas

app = FastAPI()
geo = Atlas(database_url="postgresql://user:pass@localhost/gis")

@app.on_event("startup")
async def startup() -> None:
    await geo.startup()
    await geo.register_layer("geo.municipios", title="Municipios de Jalisco")

app.include_router(geo.router)
```

Eso publica, para cada capa registrada:

| Endpoint | Qué hace |
|---|---|
| `GET /atlas/layers` | Catálogo de capas |
| `GET /atlas/layers/{layer}` | Metadatos: atributos, CRS, extent, formatos |
| `GET /atlas/layers/{layer}/download?format=geojson` | Descarga, con filtros |
| `GET /atlas/wms?REQUEST=GetMap&...` | Imagen PNG/JPEG de la capa |
| `GET /atlas/wms?REQUEST=GetFeatureInfo&...` | Atributos del elemento en un píxel |
| `GET /atlas/wfs?REQUEST=GetFeature&...` | Features en GeoJSON |
| `POST /atlas/maps/export` | Mapa compuesto como imagen |

El esquema, la columna de geometría, el SRID, la llave primaria, los atributos y el extent se
descubren solos por introspección: solo hace falta el nombre de la tabla.

## Filtros

Misma sintaxis en WFS y en descargas:

```
/atlas/layers/municipios/download?format=csv&poblacion__gt=100000&estado=Jalisco
```

Operadores: `eq` `ne` `gt` `gte` `lt` `lte` `in` `like`, más `bbox`. Los nombres de columna se
validan contra los atributos reales de la capa y los valores van parametrizados — nunca se
concatena SQL. Ver [security.md](docs/components/security.md).

## Documentación

| Documento | Para qué |
|---|---|
| [architecture.md](docs/architecture.md) | Componentes, reglas de dependencia y flujo de una petición |
| [components/](docs/components/) | Un documento por componente: contrato, invariantes, alcance |
| [security.md](docs/components/security.md) | Inyección SQL, autorización, límites de recursos |
| [conventions.md](docs/conventions.md) | Estilo de código, SQL, tests, commits |
| [roadmap.md](docs/roadmap.md) | Qué entra en cada versión y qué no entra nunca |
| [adr/](docs/adr/) | Decisiones técnicas y alternativas descartadas |
| [requirements.md](docs/requirements.md) | Manual de requerimientos original |
| [changelog.md](docs/changelog.md) | Cuándo y cómo se actualiza el CHANGELOG |
| [CHANGELOG.md](CHANGELOG.md) | Qué cambió en cada versión |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Cómo se trabaja un issue |

## Alcance

Atlas cubre lo que un geoportal usa de verdad, no el estándar OGC completo
([ADR 0003](docs/adr/0003-alcance-ogc-reducido.md)):

**Sí:** PostGIS, datos vectoriales, WMS 1.3.0 (`GetCapabilities`, `GetMap`, `GetFeatureInfo`),
WFS 2.0.0 de lectura (`GetCapabilities`, `DescribeFeatureType`, `GetFeature`), GeoJSON, CSV,
GeoPackage, Shapefile, exportación PNG/JPEG.

**No:** WFS-T, SLD, GML completo, WPS, CSW, Oracle/MySQL/ArcSDE, Shapefile como datasource,
motor propio de rendering o reproyección. WMTS, vector tiles y raster son fases posteriores.

## Licencia

MIT
