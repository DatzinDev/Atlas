# raster — contrato propuesto

**No implementado.** [ADR 0007](../adr/0007-optional-raster.md) es normativo.

- Registro y RasterSource separados de Layer/PostGIS; paths solo configurados por host.
- Modelos importan core; backend nativo es lazy y opcional; no importa WMS/WFS/postgis.
- Servicio/registro usan checker ráster y principal host; rutas solo delegan tras validación.
- Inspección, read_window, sample, PNG y XYZ delegan reproyección en GDAL.
- Sin estilo no hay PNG; metadata y sample conservan información numérica.
- Pixel/bytes/semáforo limitan trabajo; cancelación retiene turno hasta cerrar handles.
- Pruebas base/extra separadas y benchmark de estabilidad; no se anuncia WCS/WMTS.
