"""Built-in streaming exporters; optional file drivers are not imported here."""

from atlas.exports.base import EXPORTERS
from atlas.exports.csv_export import CSVExporter
from atlas.exports.geojson import GeoJSONExporter
from atlas.exports.geopackage import GeoPackageExporter
from atlas.exports.shapefile import ShapefileExporter

EXPORTERS["geojson"] = GeoJSONExporter()

EXPORTERS["csv"] = CSVExporter()

EXPORTERS["gpkg"] = GeoPackageExporter()

EXPORTERS["shp"] = ShapefileExporter()
