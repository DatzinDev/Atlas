"""WMS discovery declares only implemented operations and safely serializes metadata."""

from xml.etree import ElementTree as ET

from atlas.core.crs import SUPPORTED_CRS
from atlas.layers.models import Layer
from atlas.wms.capabilities import capabilities_xml

NS = {"w": "http://www.opengis.net/wms"}


def layer(**changes):
    return Layer.model_validate(
        dict(
            name="municipios",
            title="Municipios <Norte> & Sur",
            description='Texto <seguro> & "citado"',
            schema_name="geo",
            table_name="municipios",
            geometry_column="geom",
            geometry_type="Polygon",
            srid=4326,
            extent=(-103, 20, -101, 22),
        )
        | changes
    )


def test_document_operations_formats_and_escaping():
    xml = capabilities_xml([layer()], "https://host/atlas/wms?a=1&b=2")
    root = ET.fromstring(xml)
    assert root.attrib["version"] == "1.3.0"
    request = root.find("w:Capability/w:Request", NS)
    assert [node.tag.split("}")[1] for node in request] == [
        "GetCapabilities",
        "GetMap",
        "GetFeatureInfo",
    ]
    assert [node.text for node in request.findall("w:GetMap/w:Format", NS)] == [
        "image/png",
        "image/jpeg",
    ]
    assert request.find("w:GetFeatureInfo/w:Format", NS).text == "application/json"
    node = root.find("w:Capability/w:Layer/w:Layer", NS)
    assert node.find("w:Title", NS).text == "Municipios <Norte> & Sur"
    assert "&lt;Norte&gt; &amp; Sur" in xml
    resource = request.find(".//w:OnlineResource", NS)
    assert resource.attrib["{http://www.w3.org/1999/xlink}href"] == "https://host/atlas/wms?a=1&b=2"
    assert node.find("w:Style/w:Name", NS).text == "default"


def test_bounds_and_crs_axis_order():
    node = ET.fromstring(capabilities_xml([layer()], "/wms")).find(
        "w:Capability/w:Layer/w:Layer", NS
    )
    assert {child.text for child in node.findall("w:CRS", NS)} == {"EPSG:4326", "EPSG:3857"}
    boxes = {box.attrib["CRS"]: box.attrib for box in node.findall("w:BoundingBox", NS)}
    assert boxes["EPSG:4326"] == dict(
        CRS="EPSG:4326", minx="20.0", miny="-103.0", maxx="22.0", maxy="-101.0"
    )
    assert float(boxes["EPSG:3857"]["minx"]) == SUPPORTED_CRS[3857].bounds[0]
    assert node.find("w:EX_GeographicBoundingBox/w:westBoundLongitude", NS).text == "-103.0"


def test_disabled_layers_and_missing_extent():
    disabled = layer(name="disabled", enabled=False)
    no_wms = layer(name="no-wms", services={"wms": False})
    root = ET.fromstring(capabilities_xml([disabled, no_wms, layer(extent=None)], "/wms"))
    nodes = root.findall("w:Capability/w:Layer/w:Layer", NS)
    assert len(nodes) == 1 and nodes[0].find("w:Name", NS).text == "municipios"
    assert nodes[0].find("w:BoundingBox", NS) is None
    assert nodes[0].find("w:EX_GeographicBoundingBox", NS) is None
