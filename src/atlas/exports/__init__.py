"""Built-in streaming exporters; optional file drivers are not imported here."""

from atlas.exports.base import EXPORTERS
from atlas.exports.geojson import GeoJSONExporter

EXPORTERS["geojson"] = GeoJSONExporter()
