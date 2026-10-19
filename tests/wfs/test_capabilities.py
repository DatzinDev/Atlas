"""Capabilities advertise supported operations and XML-escaped visible layers only."""

from xml.etree import ElementTree as ET

from atlas.wfs.capabilities import capabilities_xml

NS = {"wfs": "http://www.opengis.net/wfs/2.0", "ows": "http://www.opengis.net/ows/1.1"}


async def test_operations_layers_crs_and_xml_escaping(municipios_layer, escuelas_layer):
    municipios_layer.title = "Municipios & <barrios>"
    xml = capabilities_xml(
        [municipios_layer, escuelas_layer], "https://host/prefix/atlas/wfs?a=1&b=2"
    )
    root = ET.fromstring(xml)
    assert root.attrib["version"] == "2.0.0"
    assert {
        op.attrib["name"] for op in root.findall("ows:OperationsMetadata/ows:Operation", NS)
    } == {"GetCapabilities", "DescribeFeatureType", "GetFeature"}
    features = root.findall("wfs:FeatureTypeList/wfs:FeatureType", NS)
    assert len(features) == 2
    assert features[0].findtext("wfs:Title", namespaces=NS) == municipios_layer.title
    assert features[0].findtext("wfs:DefaultCRS", namespaces=NS).endswith("4326")
    assert features[0].findtext("wfs:OtherCRS", namespaces=NS).endswith("3857")
    assert features[0].find("ows:WGS84BoundingBox", NS) is not None
    assert {f.text for f in features[0].findall("wfs:OutputFormats/wfs:Format", NS)} == {
        "application/json",
        "application/geo+json",
    }
    assert "https://host/prefix/atlas/wfs?a=1&amp;b=2" in xml


async def test_excludes_disabled_and_unsupported_layers(municipios_layer, escuelas_layer):
    municipios_layer.enabled = False
    escuelas_layer.services.wfs = False
    assert (
        ET.fromstring(capabilities_xml([municipios_layer, escuelas_layer], "https://host")).findall(
            "wfs:FeatureTypeList/wfs:FeatureType", NS
        )
        == []
    )
