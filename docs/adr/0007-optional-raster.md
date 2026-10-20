# ADR 0007 — Ráster local como extra opcional

**Estado:** propuesto para revisión; implementación pendiente (#107, #111–#139).
**Fecha:** 2026-10-19

## Decisión y dependencias

Extra `atlas-geo[raster]`: Rasterio>=1.4.4,<1.5 y NumPy>=2,<3, explícito porque se
usará directamente. Rasterio aporta GDAL; no añadir paquete Python gdal ni rio-tiler.
La biblioteca estándar/Pillow no resuelven lectura georreferenciada/reproyección; GDAL
lo hace sin un motor propio. rio-tiler añade contratos que aún no necesitamos.
Instalación/import temporales verificados en Linux x86_64, Python 3.11.14 y 3.13.11,
Rasterio 1.4.4/GDAL 3.10.3 (2026-10-19). No se afirman otras plataformas ni toda la
franja de versiones: CI probará la mínima y la resolución vigente antes de ampliar.
Este ADR no modifica pyproject ni instala dependencias en el entorno del proyecto.

`pip install atlas-geo` y `from atlas import Atlas` no requieren/cargan Rasterio ni
backend GDAL de ráster. NumPy puede existir por Shapely; no se exige su ausencia.
Modelos sin imports nativos; require_rasterio importa al usar el backend y lanza
UnsupportedFormat (400), indicando instalar atlas-geo[raster], si falta el extra.
PostGIS sigue siendo requisito de Atlas; esto no promete un host raster-only.
La reproyección vectorial sigue en ST_Transform; solo ráster usa Rasterio/GDAL.

## Fuente y autoridad

RasterSource es un modelo separado de Layer PostGIS; registro independiente por nombre.
El host configura un path local GeoTIFF/COG, bandas, estilo y permisos. Resolver path
antes de abrir; solo archivo regular local GTiff, sin URI, /vsi*, VRT ni rutas aportadas
por HTTP. Restringir drivers y accesos externos; los archivos son configuración confiable
del host, no uploads arbitrarios. No se publican paths ni errores internos de GDAL.
Atlas.register_raster(source) será async: inspección en thread, metadata en registro,
sin handles abiertos persistentes. Archivo reemplazado requiere volver a registrar;
no se descubre el catálogo leyendo disco por petición.

Checker ráster explícito, con el mismo principal del host; fuente privada falla cerrada
si falta checker. Listado/mapa/XYZ requieren view; detalle y sample requieren query.
La autorización precede a inspección/píxeles en consultas públicas; registro solo por host.
No se reutilizan permisos vectoriales por coincidencia de nombre ni se incluye ráster
en /layers. Sin frontend, usuarios, OIDC, catálogo temporal ni adaptadores de producto.

## Contratos Python propuestos

- Metadata: nombre, tamaño, número de bandas, dtype por banda, CRS/WKT, bounds xy,
  transform afín de seis coeficientes, nodata por banda y resoluciones de overviews.
  CRS ausente/fuente sin georreferencia afín se rechaza; no inventar EPSG desde WKT.
- read_window(source, *, bbox, crs, width, height, bands, resampling, limits)
  devuelve RasterWindow(data, mask, transform, crs). Bandas 1-based validadas sin duplicados;
  data nativa con shape (bandas,height,width), mask bool del mismo shape (True=inválido).
  transform/crs describen exactamente el destino, bbox xy incluso EPSG:4326, píxel centrado.
- WarpedVRT/GDAL calcula ventana/reproyección a destino 4326 o 3857, fuente con WKT nativo.
  Nada de leer todo y recortar en Python; usar bloques/overviews cuando estén disponibles.
- sample(source, *, x, y, crs, bands) devuelve list[int|float|None], nearest numérico
  de celda; fuera de extent, nodata o máscara inválida produce None por banda.
  Transformación de coordenadas en GDAL. No interpreta un PNG ni inventa unidades DEM.
- Servicio autorizado get_raster_map(ctx, name, *, bbox, crs, width, height,
  principal=None) devuelve bytes PNG. El acceso Python de bajo nivel es confiable;
  HTTP siempre pasa por servicio y checker, nunca llama directamente al lector/sample.

## Estilos, máscara y remuestreo

RasterStyle: RGB con tres bandas y min/max finitos por banda, o SingleBand con banda,
min/max y al menos dos colores explícitos. Siempre min<max; sin estilo hay metadata/sample,
pero mapa falla InvalidParameter. Rangos fijos del host, nunca autoescala por ventana/tile.
Interpolación de paleta lineal, clamp fuera del rango; RGB escala a uint8. La máscara
combina nodata, máscara GDAL/alpha y no finitos; cualquier banda visual inválida da alpha=0.
No confundir cero con nodata si la fuente no lo declara. Ventana fuera de extent: PNG transparente.
Nearest por defecto y obligatorio para clases categóricas; bilinear solo opt-in del host
para datos continuos, respetando máscara. El mismo grid/estilo produce bordes coherentes.
PNG visual pierde precisión y no es una cobertura GeoTIFF ni una descarga DEM numérica.

## HTTP opt-in y errores

Montar rutas bajo el prefijo Atlas cuando se habilite soporte ráster:
GET /rasters y GET /rasters/{name} devuelven metadata autorizada sin paths;
GET /rasters/{name}/map?bbox=minx,miny,maxx,maxy&crs=EPSG:3857&width=256&height=256;
GET /rasters/{name}/tiles/{z}/{x}/{y}.png usa el grid core del ADR 0006, lado fijo 256.
Mapas/tiles: 200 image/png y Cache-Control:no-store; no estilos ni fuentes por query.
InvalidParameter/UnsupportedFormat 400, denegación 403, fuente inexistente 404,
QueryTooLarge 413; errores nativos se traducen a Atlas sin revelar path/credenciales.
No son rutas WMS/WCS/WMTS; no anunciar esas capacidades en GetCapabilities.

## Recursos y cancelación

AtlasConfig: max_raster_pixels=4194304 (width*height), max_raster_bytes=67108864
(data+mask estimados por bandas/dtype), max_raster_concurrency=1 por proceso.
Validar límites positivos, dimensiones, CRS, bandas y bytes antes de asignar; no reducir
resolución ni truncar silenciosamente. Presupuestos son distintos de la paginación vectorial.
Context managers cierran dataset/VRT por lectura, incluso si falla. Trabajo nativo fuera
del event loop bajo semáforo compartido; cancelar la espera no libera turno hasta que
termine realmente el thread y cierre handles. El shutdown drena trabajo activo.
Limitar caché GDAL a 64 MiB por proceso; bloques, warp y encoder añaden memoria fuera
de data+mask, así que estos límites no garantizan RSS constante ni interrumpen un GDAL
bloqueado. El host limita concurrencia/colas y recursos del proceso según medición.

## Verificación y ampliaciones diferidas

CI base sin Rasterio y CI extra con lectura/PNG reales; no contar tests omitidos como éxito.
Fixtures numéricas para affine/CRS, máscaras, nearest/bilinear, clases, overviews, límites,
permisos y cancelación. Benchmark #139: tres repeticiones y soak 30 min, RSS/handles/I/O.
COG acelera ventanas solo según bloques/overviews; no garantiza menor I/O en todo archivo.
Se difieren HTTP/S3, PostGIS Raster, WCS, DEM-RGB, calendarios y seeding.

Fuentes: [ventanas](https://rasterio.readthedocs.io/en/stable/topics/windowed-rw.html),
[WarpedVRT](https://rasterio.readthedocs.io/en/stable/topics/virtual-warping.html).
