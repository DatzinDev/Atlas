# Manual de Requerimientos
## Librería Python para publicación de capas PostGIS en aplicaciones web geoespaciales

**Versión:** 0.1  
**Estado:** Definición inicial  
**Tipo de proyecto:** Librería Python / Toolkit geoespacial  
**Objetivo principal:** Facilitar la publicación, visualización, consulta y descarga de capas almacenadas en PostGIS desde aplicaciones Python, especialmente FastAPI, sin depender de GeoServer.

---

# 1. Introducción

El proyecto consiste en desarrollar una librería Python orientada a aplicaciones geoespaciales que permita registrar tablas y vistas de PostGIS como capas consumibles desde aplicaciones web.

La librería deberá cubrir las necesidades principales de un geoportal:

- Visualización de capas geográficas.
- Consulta de atributos y geometrías.
- Interacción con elementos del mapa.
- Descarga de datos para análisis externo.
- Exportación de mapas como imágenes.
- Gestión centralizada de capas desde el backend.
- Integración directa con aplicaciones FastAPI.

El objetivo no es construir un reemplazo completo de GeoServer ni implementar la totalidad de los estándares OGC.

La librería deberá enfocarse únicamente en las capacidades necesarias para aplicaciones web geoespaciales basadas principalmente en PostGIS.

---

# 2. Principios del proyecto

## 2.1 Alcance limitado

El proyecto no deberá intentar convertirse en un servidor GIS universal.

El alcance inicial estará limitado a:

- PostgreSQL.
- PostGIS.
- Datos vectoriales.
- Aplicaciones Python.
- FastAPI.
- WMS.
- WFS.
- Exportación de datos.

El MVP vectorial se conserva. Se aprueban MVT nativo PostGIS y un extra opcional de
GeoTIFF/COG local; sus contratos se definirán antes de implementarse. WMTS y WCS
completos, fuentes HTTP/S3 y PostGIS Raster quedan diferidos.

---

## 2.2 PostGIS como fuente principal

PostGIS será considerado la fuente de datos geoespaciales principal.

No será requisito inicial soportar:

- Oracle Spatial.
- MySQL Spatial.
- ArcSDE.
- Shapefile como datasource.
- GeoPackage como datasource.
- Servicios WFS externos.
- Fuentes propietarias.

Otros datasources podrán ser agregados posteriormente mediante interfaces o adaptadores.

---

# 3. Objetivos

## 3.1 Objetivo general

Permitir que un desarrollador registre una tabla o vista PostGIS y obtenga automáticamente los endpoints necesarios para visualizar, consultar y descargar dicha información desde una aplicación web.

Ejemplo conceptual:

```python
geo.register_layer(
    name="municipios",
    table="geo.municipios",
    geometry_column="geom",
)
```

A partir de esta configuración, la aplicación deberá poder exponer:

```text
WMS
WFS
API de metadatos
Descargas
```

sin necesidad de configurar un servidor GIS externo.

---

## 3.2 Objetivos específicos

La librería deberá:

1. Conectarse a PostgreSQL/PostGIS.
2. Detectar información espacial de tablas y vistas.
3. Registrar recursos como capas.
4. Exponer dichas capas mediante WMS.
5. Exponer dichas capas mediante WFS.
6. Permitir obtener metadatos de las capas mediante una API.
7. Permitir descargar los datos en formatos comunes.
8. Permitir exportar visualizaciones como PNG.
9. Permitir definir estilos básicos.
10. Integrarse con los sistemas de autenticación y autorización de la aplicación host.
11. Ser utilizable como librería y no requerir necesariamente un servicio independiente.

---

# 4. Casos de uso principales

## CU-01 — Visualizar una capa

Un usuario activa una capa desde el geoportal.

El frontend solicita una imagen mediante WMS.

```text
Frontend
    ↓
WMS GetMap
    ↓
Backend
    ↓
PostGIS
    ↓
Renderer
    ↓
PNG
```

---

## CU-02 — Consultar un elemento del mapa

El usuario hace clic sobre una geometría.

El sistema deberá poder devolver sus atributos mediante:

```text
WMS GetFeatureInfo
```

o mediante WFS.

---

## CU-03 — Consultar geometrías

El frontend solicita las geometrías y atributos de una capa.

```text
WFS GetFeature
```

El servidor consulta PostGIS y devuelve GeoJSON.

---

## CU-04 — Filtrar información

El usuario aplica filtros desde el geoportal.

Ejemplo:

```text
poblacion > 100000
```

El backend deberá traducir el filtro a una consulta segura sobre PostGIS.

---

## CU-05 — Descargar una capa

El usuario selecciona:

```text
Descargar capa
```

y elige un formato.

Inicialmente deberán soportarse:

- GeoJSON.
- CSV.
- GeoPackage.
- Shapefile ZIP.

---

## CU-06 — Descargar un mapa como imagen

El usuario selecciona:

```text
Descargar mapa como PNG
```

El backend deberá generar una imagen WMS utilizando:

- Capas seleccionadas.
- Extensión geográfica actual.
- Estilos activos.
- Resolución solicitada.

---

# 5. Arquitectura conceptual

```text
                    Aplicación FastAPI
                           │
             ┌─────────────┼──────────────┐
             │             │              │
        Layer API         WMS            WFS
             │             │              │
             └─────────────┼──────────────┘
                           │
                    Layer Registry
                           │
             ┌─────────────┼─────────────┐
             │             │             │
          Queries       Rendering      Export
             │             │             │
             └─────────────┼─────────────┘
                           │
                        PostGIS
```

---

# 6. Integración con FastAPI

La librería deberá poder integrarse dentro de una aplicación existente.

Ejemplo conceptual:

```python
from fastapi import FastAPI
from geo_toolkit import GeoServer

app = FastAPI()

geo = GeoServer(
    database_url="postgresql://..."
)

geo.register_layer(
    name="municipios",
    table="geo.municipios",
    geometry_column="geom",
)

app.include_router(geo.router)
```

La librería no deberá requerir obligatoriamente ejecutar un proceso o servidor adicional.

---

# 7. Layer Registry

El componente central del sistema será el registro de capas.

Cada recurso vectorial PostGIS publicado se representa mediante `Layer`. Las futuras
fuentes ráster tendrán un modelo y registro separados, sin simular una tabla PostGIS.

---

## 7.1 Propiedades mínimas

Una capa deberá contener:

```text
id
name
title
description
schema
table
geometry_column
geometry_type
srid
enabled
searchable
downloadable
```

---

## 7.2 Servicios habilitados

Cada capa deberá poder definir individualmente qué servicios ofrece.

Ejemplo:

```python
Layer(
    name="municipios",

    wms=True,
    wfs=True,
    wmts=False,
    wcs=False,
)
```

---

## 7.3 Configuración conceptual

```python
geo.register_layer(
    name="municipios",
    title="Municipios de Jalisco",
    table="geo.municipios",
    geometry_column="geom",
    id_column="id",
    searchable=True,
    downloadable=True,
    services={
        "wms": True,
        "wfs": True,
    },
)
```

---

# 8. Persistencia del catálogo

La aplicación podrá opcionalmente mantener el catálogo de capas en PostgreSQL.

Tabla conceptual:

```text
geo_layers
──────────────────────────
id
name
title
description

schema
table
geometry_column
geometry_type
srid

style

wms_enabled
wfs_enabled
wmts_enabled
wcs_enabled

downloadable
searchable
public

min_zoom
max_zoom

created_at
updated_at
```

Esto permitirá crear interfaces administrativas para publicar y despublicar capas dinámicamente.

---

# 9. Requerimientos WMS

La versión inicial deberá implementar únicamente las operaciones necesarias para un geoportal.

---

## RF-WMS-01 — GetCapabilities

El sistema deberá implementar:

```text
REQUEST=GetCapabilities
```

Deberá devolver como mínimo:

- Versión del servicio.
- Capas disponibles.
- Nombre.
- Título.
- CRS disponibles.
- Bounding box.
- Formatos de imagen.
- Estilos disponibles.

---

## RF-WMS-02 — GetMap

El sistema deberá implementar:

```text
REQUEST=GetMap
```

Parámetros mínimos:

```text
LAYERS
BBOX
WIDTH
HEIGHT
CRS
FORMAT
STYLES
TRANSPARENT
```

Formatos iniciales:

```text
image/png
image/jpeg
```

---

## RF-WMS-03 — GetFeatureInfo

Deberá ser posible consultar features visibles en una posición determinada del mapa.

El resultado deberá poder incluir:

- Identificador.
- Atributos.
- Geometría opcional.

Formato recomendado:

```text
application/json
```

---

# 10. Renderizado WMS

La librería no deberá implementar un motor gráfico desde cero.

El componente de renderizado deberá abstraerse mediante una interfaz.

Ejemplo conceptual:

```python
class MapRenderer:

    def render(
        self,
        layers,
        bbox,
        width,
        height,
        crs,
    ) -> bytes:
        ...
```

Esto permitirá utilizar implementaciones externas como:

- Mapnik.
- GDAL.
- Otros renderers futuros.

---

# 11. Sistema de estilos

No será requisito inicial implementar SLD completo.

Se utilizará un modelo de estilos propio y simplificado.

---

## 11.1 Estilo de polígonos

```json
{
  "fill": "#3178c6",
  "fill_opacity": 0.7,
  "stroke": "#ffffff",
  "stroke_width": 1
}
```

---

## 11.2 Estilo de líneas

```json
{
  "stroke": "#3178c6",
  "stroke_width": 2,
  "stroke_opacity": 1
}
```

---

## 11.3 Estilo de puntos

```json
{
  "radius": 5,
  "fill": "#3178c6",
  "stroke": "#ffffff"
}
```

---

## 11.4 Etiquetas

Deberán considerarse posteriormente propiedades como:

```text
field
font
size
color
halo
```

No deberán formar parte obligatoria del primer prototipo.

---

# 12. Requerimientos WFS

La implementación inicial será de solo lectura.

---

## RF-WFS-01 — GetCapabilities

Deberá devolver:

- Capas disponibles.
- Operaciones soportadas.
- Formatos.
- CRS.

---

## RF-WFS-02 — DescribeFeatureType

Permitirá obtener el esquema de una capa.

Debe incluir:

- Nombre de campos.
- Tipos.
- Campo geométrico.
- Tipo geométrico.

---

## RF-WFS-03 — GetFeature

Permitirá consultar features.

Formato principal:

```text
GeoJSON
```

---

## RF-WFS-04 — Bounding Box

Deberán poder limitarse resultados mediante:

```text
bbox=minx,miny,maxx,maxy
```

La consulta deberá resolverse espacialmente desde PostGIS.

---

## RF-WFS-05 — Paginación

Las consultas deberán soportar paginación.

Conceptualmente:

```text
limit
offset
```

o sus equivalentes WFS.

---

## RF-WFS-06 — Selección de propiedades

El cliente deberá poder solicitar únicamente determinados campos.

Ejemplo:

```text
nombre
clave
poblacion
```

para evitar transferencias innecesarias.

---

# 13. Filtros

La librería deberá permitir filtrado de features.

Ejemplos:

```text
municipio = 'Guadalajara'
```

```text
poblacion > 100000
```

```text
anio = 2025
```

---

## 13.1 Seguridad

Los filtros nunca deberán concatenarse directamente en consultas SQL.

Deberán utilizarse:

- Queries parametrizadas.
- Lista blanca de columnas.
- Lista blanca de operadores.

---

## 13.2 Operadores iniciales

```text
=
!=
>
>=
<
<=
IN
LIKE
```

Los operadores espaciales podrán agregarse posteriormente.

---

# 14. API propia de capas

Además de OGC, la librería deberá ofrecer una API más cómoda para los frontends.

Ejemplo:

```text
GET /layers
GET /layers/{layer}
```

---

## RF-LAYER-01 — Listado

```http
GET /layers
```

deberá devolver el catálogo disponible.

---

## RF-LAYER-02 — Detalle

```http
GET /layers/municipios
```

deberá devolver información como:

```json
{
  "name": "municipios",
  "title": "Municipios de Jalisco",

  "geometry_type": "Polygon",
  "srid": 4326,

  "attributes": [
    "clave",
    "nombre",
    "poblacion"
  ],

  "services": {
    "wms": true,
    "wfs": true
  },

  "download_formats": [
    "geojson",
    "csv",
    "gpkg",
    "shapefile"
  ]
}
```

---

# 15. Descarga de datos

La descarga de datos no tendrá que depender exclusivamente de WFS.

La librería ofrecerá endpoints propios.

Ejemplo:

```text
GET /layers/{layer}/download
```

---

## 15.1 Formatos requeridos

### MVP

```text
GeoJSON
CSV
```

### Posteriores

```text
GeoPackage
Shapefile ZIP
```

---

## 15.2 Descargas filtradas

El sistema deberá permitir descargar únicamente los resultados correspondientes a un filtro.

Ejemplo:

```text
/layers/municipios/download
    ?format=geojson
    &poblacion__gt=100000
```

---

## 15.3 Descarga por extensión

Deberá contemplarse posteriormente:

```text
bbox
```

para descargar únicamente features dentro de una región determinada.

---

# 16. Exportación de mapas

La aplicación deberá permitir generar imágenes independientes del visor web.

Endpoint conceptual:

```text
POST /maps/export
```

Entrada:

```json
{
  "layers": [
    "municipios",
    "carreteras"
  ],

  "bbox": [
    -104,
    19,
    -102,
    21
  ],

  "width": 1920,
  "height": 1080,

  "format": "png"
}
```

El resultado será una imagen.

Formatos iniciales:

```text
PNG
JPEG
```

---

# 17. Integración con autenticación

La librería no deberá implementar un sistema propio de usuarios.

La aplicación host será responsable de:

- Autenticación.
- Roles.
- Permisos.

La librería deberá permitir integrar dichas reglas.

Ejemplo conceptual:

```python
geo.register_layer(
    name="predios",
    permissions=[
        "geo.predios.read"
    ],
)
```

---

# 18. Autorización por capa

Deberá ser posible controlar:

```text
visualizar
consultar
descargar
administrar
```

por separado.

Ejemplo conceptual:

```text
geo.predios.view
geo.predios.query
geo.predios.download
geo.predios.admin
```

---

# 19. Introspección de PostGIS

La librería deberá ser capaz de obtener automáticamente metadatos de las tablas.

Como mínimo:

```text
schema
table
geometry_column
geometry_type
SRID
columns
primary_key
extent
```

Esto permitirá reducir la configuración manual.

Ejemplo deseado:

```python
geo.register_layer(
    "geo.municipios"
)
```

y que la librería descubra:

```text
geom
Polygon
EPSG:4326
id
atributos
extent
```

automáticamente.

---

# 20. Gestión de CRS

La versión inicial deberá soportar como mínimo:

```text
EPSG:4326
EPSG:3857
```

La arquitectura deberá permitir agregar otros sistemas posteriormente.

PostGIS deberá utilizarse siempre que sea posible para transformaciones mediante:

```text
ST_Transform
```

---

# 21. Bounding boxes

La librería deberá poder calcular automáticamente la extensión geográfica de cada capa.

Esta información se utilizará para:

- WMS GetCapabilities.
- Zoom automático.
- Previsualización.
- Validación de consultas.

---

# 22. Rendimiento

Las consultas espaciales deberán aprovechar índices PostGIS.

Las geometrías deberán almacenarse con índices:

```sql
CREATE INDEX
ON tabla
USING GIST (geom);
```

Las consultas espaciales deberán utilizar operadores compatibles con dichos índices.

---

# 23. Límites de respuesta

La librería deberá impedir que una petición accidental descargue millones de geometrías sin restricciones.

Configuración sugerida:

```python
GeoConfig(
    default_limit=1000,
    max_limit=10000,
)
```

Las descargas completas podrán utilizar mecanismos diferentes.

---

# 24. Streaming

Las descargas grandes deberán poder enviarse mediante streaming para evitar cargar todo el dataset en memoria.

Especialmente:

```text
CSV
GeoJSON
GeoPackage
Shapefile
```

cuando resulte viable.

---

# 25. Logging

La librería deberá permitir integrar el sistema de logging de la aplicación.

Información relevante:

```text
layer
operation
request
execution_time
feature_count
user
error
```

---

# 26. Manejo de errores

La librería deberá proporcionar errores consistentes para:

```text
capa inexistente
CRS no soportado
formato no soportado
filtro inválido
geometría inválida
consulta demasiado grande
timeout
problemas con PostGIS
```

---

# 27. Observabilidad

Posteriormente podrán añadirse métricas como:

```text
wms_requests_total

wfs_requests_total

layer_requests_total

query_duration_seconds

render_duration_seconds

download_size_bytes
```

---

# 28. Caché

El caché no será obligatorio para V1.

La arquitectura deberá permitir incorporarlo posteriormente.

Niveles posibles:

```text
metadata cache
query cache
image cache
tile cache
```

Backends posibles:

```text
memory
filesystem
Redis
object storage
```

---

# 29. WMTS — Fase posterior

WMTS no será necesario para el MVP.

Será implementado cuando:

- Aumente el tráfico.
- Existan capas relativamente estáticas.
- WMS genere demasiado trabajo de renderizado.
- Sea necesario cachear tiles.

Operaciones iniciales futuras:

```text
GetCapabilities
GetTile
```

---

# 30. Vector Tiles — Contrato aprobado, implementación pendiente

La librería podrá ofrecer tiles MVT directamente desde PostGIS.

Ruta conceptual:

```text
/tiles/{name}/{z}/{x}/{y}.mvt
```

PostGIS podrá generar los datos mediante:

```text
ST_TileEnvelope
ST_AsMVTGeom
ST_AsMVT
```

---

# 31. WCS — Fase posterior

WCS se agregará cuando exista necesidad real de trabajar con raster.

Casos:

```text
DEM
imágenes satelitales
NDVI
temperatura
precipitación
ortofotos
```

---

## 31.1 Alcance ráster aprobado

Extra opcional GeoTIFF/COG local con Rasterio/GDAL y API REST opt-in de mapas,
XYZ y metadata. No implica WCS. La instalación vectorial no requiere Rasterio;
PostGIS sigue siendo requisito del Atlas actual, sin prometer ejecución raster-only.
La reproyección vectorial usa ST_Transform; la ráster delega en GDAL.

Backend posterior, fuera del primer soporte: PostGIS Raster.

---

## 31.2 Operaciones WCS

Inicialmente:

```text
GetCapabilities
DescribeCoverage
GetCoverage
```

---

# 32. Fuera de alcance inicial

No deberán formar parte del MVP:

```text
WFS-T

WFS LockFeature

WFS Stored Queries

SLD completo

GML completo

WPS

CSW

Oracle Spatial

MySQL Spatial

ArcSDE

Shapefile como datasource

GeoPackage como datasource

edición remota

cartografía avanzada

motor propio de reproyección

motor propio de rendering

raster processing propio
```

---

# 33. Organización conceptual del paquete

```text
geo_toolkit/
│
├── core/
│   ├── config.py
│   ├── exceptions.py
│   └── types.py
│
├── layers/
│   ├── models.py
│   ├── registry.py
│   ├── metadata.py
│   └── permissions.py
│
├── postgis/
│   ├── connection.py
│   ├── introspection.py
│   ├── queries.py
│   └── geometry.py
│
├── wms/
│   ├── router.py
│   ├── capabilities.py
│   ├── get_map.py
│   └── feature_info.py
│
├── wfs/
│   ├── router.py
│   ├── capabilities.py
│   ├── describe.py
│   ├── get_feature.py
│   └── filters.py
│
├── styles/
│   ├── models.py
│   ├── polygon.py
│   ├── line.py
│   ├── point.py
│   └── compiler.py
│
├── rendering/
│   ├── base.py
│   └── mapnik.py
│
├── exports/
│   ├── geojson.py
│   ├── csv.py
│   ├── geopackage.py
│   └── shapefile.py
│
├── api/
│   ├── layers.py
│   ├── download.py
│   └── maps.py
│
└── fastapi.py
```

Esta estructura es únicamente conceptual y podrá cambiar durante el diseño.

---

# 34. Fases recomendadas

## Fase 0 — Spike técnico

Antes de construir la librería completa:

1. Conectar Python con PostGIS.
2. Consultar una capa.
3. Obtener GeoJSON.
4. Renderizar un PNG.
5. Integrarlo en FastAPI.

Objetivo:

Validar que el stack seleccionado funciona correctamente.

---

# 35. Fase 1 — Núcleo

Implementar:

```text
PostGIS connection

Layer

LayerRegistry

introspection

metadata

FastAPI integration
```

Resultado esperado:

```python
geo.register_layer(...)
```

debe funcionar correctamente.

---

# 36. Fase 2 — WFS

Implementar:

```text
GetCapabilities
DescribeFeatureType
GetFeature
bbox
pagination
GeoJSON
filters
```

Resultado:

El geoportal puede consultar y descargar datos vectoriales.

---

# 37. Fase 3 — WMS

Implementar:

```text
GetCapabilities
GetMap
GetFeatureInfo
```

Más:

```text
polygon styles
line styles
point styles
PNG
JPEG
```

Resultado:

El geoportal puede visualizar las capas.

---

# 38. Fase 4 — Exportación

Implementar:

```text
GeoJSON
CSV
GeoPackage
Shapefile ZIP
```

Más endpoint:

```text
/layers/{layer}/download
```

---

# 39. Fase 5 — Administración

Agregar un catálogo persistente.

Permitir:

```text
registrar capa
editar capa
activar/desactivar
cambiar título
cambiar estilo
configurar descarga
configurar permisos
```

Esto permitirá eventualmente construir una GUI administrativa.

---

# 40. Etapas activas — Contratos, consultas y MVT

El [roadmap](roadmap.md) y el índice #100 ordenan contratos, consultas compartidas,
MVT nativo y validación reproducible. Administración no es un prerrequisito.
No se añade infraestructura ni se elevan límites globales para simular capacidad.

---

# 41. Etapas activas — Ráster opcional

Primero contrato y extra local; después fuentes/ventanas, API y benchmarks.
Sin rio-tiler por defecto ni promesa de WCS, PostGIS Raster o lectura remota.

---

# 42. Criterios de éxito del MVP

El MVP deberá considerarse exitoso cuando sea posible realizar el siguiente flujo:

### 1. Registrar

```python
geo.register_layer(
    "geo.municipios"
)
```

### 2. Visualizar

```text
WMS GetMap
```

### 3. Consultar

```text
WMS GetFeatureInfo
```

### 4. Obtener datos

```text
WFS GetFeature
```

### 5. Filtrar

```text
poblacion > 100000
```

### 6. Descargar

```text
GeoJSON
CSV
```

### 7. Exportar

```text
PNG
```

Todo esto deberá funcionar:

```text
PostGIS
   ↓
misma aplicación Python/FastAPI
   ↓
frontend
```

sin levantar GeoServer.

---

# 43. Criterios de no-éxito

El proyecto estará desviándose de su objetivo si empieza a requerir implementar:

```text
todas las versiones de WFS
todas las versiones de WMS
SLD completo
GML completo
edición WFS-T
múltiples gestores de base de datos
drivers GIS genéricos
plugin system complejo
administración de usuarios propia
motor de rendering propio
motor raster propio
```

Si aparece uno de estos requerimientos deberá evaluarse explícitamente antes de incorporarlo.

---

# 44. Definición resumida del producto

> Librería Python orientada a FastAPI que permite registrar tablas y vistas PostGIS como capas geográficas y publicarlas mediante WMS y WFS, además de proporcionar APIs de consulta, filtrado, descarga y exportación de mapas para aplicaciones web geoespaciales.

Su propósito es cubrir las necesidades habituales de un geoportal sin incorporar la complejidad de un servidor GIS de propósito general.

---

# 45. Principio rector

Toda nueva funcionalidad deberá responder a la pregunta:

> ¿Es una primitiva geoespacial reutilizable para publicar, visualizar, consultar o descargar datos desde una aplicación web?

Si la respuesta es no, probablemente no pertenece al núcleo del proyecto.

La prioridad deberá mantenerse en:

```text
PostGIS
    ↓
Python
    ↓
FastAPI
    ↓
WMS / WFS / API
    ↓
Geoportal
```

antes de ampliar el proyecto hacia otras capacidades.

## Frontera entre librería y aplicación

Atlas no implementa OIDC, usuarios, catálogo de producto, calendarios, ENV de un
geoportal, IDs GeoServer ni adaptadores Mapalab. El host controla autenticación,
autorización y despliegue. Se conserva feature → develop → main. Los ejemplos
conceptuales del MVP no anuncian las capacidades nuevas como implementadas.
