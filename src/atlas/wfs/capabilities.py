"""Build the capabilities document for Atlas's read-only WFS subset."""

from xml.etree.ElementTree import Element, SubElement, tostring

from atlas.core.crs import SUPPORTED_CRS
from atlas.layers.models import Layer


def capabilities_xml(layers: list[Layer], base_url: str, *, title: str = "Atlas WFS") -> str:
    """Serialize an already permission-filtered catalog; disabled WFS layers are excluded."""
    root = Element(
        "wfs:WFS_Capabilities",
        {
            "xmlns:wfs": "http://www.opengis.net/wfs/2.0",
            "xmlns:ows": "http://www.opengis.net/ows/1.1",
            "xmlns:xlink": "http://www.w3.org/1999/xlink",
            "version": "2.0.0",
        },
    )
    identification = SubElement(root, "ows:ServiceIdentification")
    SubElement(identification, "ows:Title").text = title
    SubElement(identification, "ows:ServiceType").text = "WFS"
    SubElement(identification, "ows:ServiceTypeVersion").text = "2.0.0"
    operations = SubElement(root, "ows:OperationsMetadata")
    for name in ("GetCapabilities", "DescribeFeatureType", "GetFeature"):
        operation = SubElement(operations, "ows:Operation", {"name": name})
        http = SubElement(SubElement(operation, "ows:DCP"), "ows:HTTP")
        SubElement(http, "ows:Get", {"xlink:href": base_url})
        post = SubElement(http, "ows:Post", {"xlink:href": base_url})
        constraint = SubElement(post, "ows:Constraint", {"name": "PostEncoding"})
        SubElement(SubElement(constraint, "ows:AllowedValues"), "ows:Value").text = "KVP"
    feature_types = SubElement(root, "wfs:FeatureTypeList")
    for layer in layers:
        if not layer.supports("wfs"):
            continue
        feature = SubElement(feature_types, "wfs:FeatureType")
        SubElement(feature, "wfs:Name").text = layer.name
        SubElement(feature, "wfs:Title").text = layer.title
        SubElement(feature, "wfs:Abstract").text = layer.description or ""
        SubElement(feature, "wfs:DefaultCRS").text = "urn:ogc:def:crs:EPSG::4326"
        for srid in SUPPORTED_CRS:
            if srid != 4326:
                SubElement(feature, "wfs:OtherCRS").text = f"urn:ogc:def:crs:EPSG::{srid}"
        formats = SubElement(feature, "wfs:OutputFormats")
        for format_name in ("application/json", "application/geo+json"):
            SubElement(formats, "wfs:Format").text = format_name
        if layer.extent is not None:
            box = SubElement(feature, "ows:WGS84BoundingBox")
            SubElement(box, "ows:LowerCorner").text = f"{layer.extent[0]} {layer.extent[1]}"
            SubElement(box, "ows:UpperCorner").text = f"{layer.extent[2]} {layer.extent[3]}"
    return '<?xml version="1.0" encoding="UTF-8"?>' + tostring(root, encoding="unicode")
