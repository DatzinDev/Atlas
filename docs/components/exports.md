# exports

> Módulo: `src/atlas/exports/`

## Propósito

Serializa el resultado de una consulta a formatos descargables. GeoJSON y CSV en el MVP;
GeoPackage y Shapefile ZIP requieren el extra opcional `atlas-geo[export]` (§15.1).

## Contrato público

```python
# exports/base.py
class Exporter(Protocol):
    format: ClassVar[str]              # "geojson" | "csv" | "gpkg" | "shp"
    media_type: ClassVar[str]
    extension: ClassVar[str]
    streaming: ClassVar[bool]
    def export(self, rows: AsyncIterator[dict], layer: Layer) -> AsyncIterator[bytes]: ...

EXPORTERS: dict[str, Exporter]
def get_exporter(format: str) -> Exporter      # lanza UnsupportedFormat
def available_formats() -> list[str]
def download_filename(layer: Layer, exporter: Exporter) -> str

# exports/geojson.py   GeoJSONExporter   streaming=True
# exports/csv_export.py CSVExporter      streaming=True   (geometría como WKT)
# exports/geopackage.py GeoPackageExporter streaming=False (archivo temporal + fiona/GDAL)
# exports/shapefile.py  ShapefileExporter  streaming=False (temporal + ZIP)
```

## Invariantes

- GeoJSON y CSV **se emiten en streaming**: se abre el `FeatureCollection`, se van escribiendo
  features desde un cursor server-side y se cierra. Nunca se acumula el dataset en memoria (§24).
- GeoPackage conserva el nombre de capa, SRID y los tipos compatibles (enteros de 32/64 bits,
  reales, booleanos, fecha, datetime y binarios). Numeric exacto y tipos no portables se guardan
  como texto, sin redondear su contenido; JSON y arrays se representan como JSON. El FID interno
  tiene un nombre separado para no perder atributos llamados `fid`.
- GeoPackage y Shapefile necesitan archivo en disco: se escriben a un temporal y se sirven; el
  temporal se borra siempre, incluso si el cliente aborta la descarga.
- Shapefile produce `.shp`, `.shx`, `.dbf`, `.prj`, `.cpg` (UTF-8) y `README.txt` en un ZIP.
  Los nombres se normalizan a ASCII, se truncan a 10 caracteres y se desambiguan sin distinguir
  mayúsculas; el README conserva la correspondencia completa. Int8/numeric, datetime, arrays y
  tipos no portables pasan a texto para evitar pérdida de precisión o de contenido.
  Se rechazan textos mayores de 254 bytes UTF-8 y valores fuera del rango numérico DBF.
- Shapefile tiene límites del formato que no se pueden ocultar: nombres de campo truncados a 10
  caracteres, 255 columnas y 2 GB por dataset/ZIP. Los excesos devuelven `QueryTooLarge` antes
  de enviar headers. La respuesta HTTP incorpora `Warning` con referencia al README.
- Todo exportador declara su `media_type` y `extension`; el `Content-Disposition` se construye
  desde ahí, con el nombre de la capa saneado.
- El módulo se llama `csv_export.py`, no `csv.py`, para no sombrear el módulo estándar.
- Las filas reciben geometría como objeto GeoJSON bajo `layer.geometry_column`; la metadata
  indica el SRID de salida y los atributos seleccionados. GeoJSON usa `default=str` para fechas
  y Decimal; CSV escribe BOM UTF-8 y permite WKT, centroide o ninguna geometría.
- CSV antepone una comilla simple a texto/cabeceras que pueden interpretarse como fórmulas
  (también tras espacios), incluidos prefijos de control y variantes Unicode. No altera números
  tipados, NULL ni las fuentes. Este escape cambia el texto exportado; para intercambio exacto
  usar GeoJSON/GPKG. No garantiza seguridad si otra aplicación elimina el escape al regrabar.
- Los exportadores no consultan la base de datos: reciben un `AsyncIterator[dict]` ya construido
  por `postgis/`.

## Dependencias

Importa: `core`, `layers` (solo tipos). Externas: `fiona` o GDAL solo para GPKG/SHP, y son
dependencias **opcionales** (`pip install atlas-geo[export]`).
Lo importan: `api`.

## Fuera de alcance

- Endpoints HTTP — viven en `api/download.py`.
- Construir el SQL — vive en `postgis/queries.py`.
- KML, GML, DXF, File Geodatabase.
- Trabajos de exportación asíncronos con notificación. Si una descarga tarda tanto que lo
  necesita, es una señal para v0.3, no para el MVP.

## Referencias

`requirements.md` §15 (descarga de datos), §15.1 (formatos), §15.2 (descargas filtradas),
§24 (streaming).
