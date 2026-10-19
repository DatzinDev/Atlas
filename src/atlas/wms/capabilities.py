"""Serialize the discoverable WMS 1.3.0 subset from a permission-filtered catalog."""

from xml.etree.ElementTree import Element, SubElement, tostring

from atlas.core.crs import SUPPORTED_CRS
from atlas.layers.models import Layer


def capabilities_xml(layers: list[Layer], base_url: str, *, title: str = "Atlas WMS") -> str:
    root = Element(
        "WMS_Capabilities",
        {
            "xmlns": "http://www.opengis.net/wms",
            "xmlns:xlink": "http://www.w3.org/1999/xlink",
            "version": "1.3.0",
        },
    )
    service = SubElement(root, "Service")
    SubElement(service, "Name").text = "WMS"
    SubElement(service, "Title").text = title
    SubElement(service, "OnlineResource", {"xlink:type": "simple", "xlink:href": base_url})
    capability = SubElement(root, "Capability")
    operations = SubElement(capability, "Request")
    formats = {
        "GetCapabilities": ("application/xml",),
        "GetMap": ("image/png", "image/jpeg"),
        "GetFeatureInfo": ("application/json",),
    }
    for name, supported in formats.items():
        operation = SubElement(operations, name)
        for image_format in supported:
            SubElement(operation, "Format").text = image_format
        get = SubElement(SubElement(SubElement(operation, "DCPType"), "HTTP"), "Get")
        SubElement(get, "OnlineResource", {"xlink:type": "simple", "xlink:href": base_url})
    parent = SubElement(capability, "Layer")
    SubElement(parent, "Title").text = title
    for layer in layers:
        if not layer.supports("wms"):
            continue
        node = SubElement(parent, "Layer", {"queryable": "1" if layer.searchable else "0"})
        SubElement(node, "Name").text = layer.name
        SubElement(node, "Title").text = layer.title
        if layer.description:
            SubElement(node, "Abstract").text = layer.description
        for info in SUPPORTED_CRS.values():
            SubElement(node, "CRS").text = info.code
        if layer.extent is not None:
            geographic = SubElement(node, "EX_GeographicBoundingBox")
            for name, value in zip(
                (
                    "westBoundLongitude",
                    "southBoundLatitude",
                    "eastBoundLongitude",
                    "northBoundLatitude",
                ),
                layer.extent,
                strict=True,
            ):
                SubElement(geographic, name).text = str(value)
            for info in SUPPORTED_CRS.values():
                # Native metadata holds geographic bounds only; other CRSs use conservative bounds.
                bounds = layer.extent if info.srid == 4326 else info.bounds
                if info.axis_order == "yx":
                    bounds = (bounds[1], bounds[0], bounds[3], bounds[2])
                SubElement(
                    node,
                    "BoundingBox",
                    {
                        "CRS": info.code,
                        **dict(
                            zip(("minx", "miny", "maxx", "maxy"), map(str, bounds), strict=True)
                        ),
                    },
                )
        style = SubElement(node, "Style")
        SubElement(style, "Name").text = "default"
        SubElement(style, "Title").text = "Layer style"
    return '<?xml version="1.0" encoding="UTF-8"?>' + tostring(root, encoding="unicode")
