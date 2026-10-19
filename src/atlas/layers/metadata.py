"""Build publishable layers from discovered PostGIS metadata and explicit overrides."""

from typing import Any

from atlas.layers.models import Layer
from atlas.postgis.connection import Database
from atlas.postgis.introspection import inspect_table, parse_source, table_extent
from atlas.styles.models import default_style


async def build_layer(
    db: Database, source: str, *, name: str | None = None, **overrides: Any
) -> Layer:
    unknown = overrides.keys() - Layer.model_fields.keys()
    if unknown:
        raise TypeError(f"unknown layer fields: {', '.join(sorted(unknown))}")
    schema, table = parse_source(source)
    info = await inspect_table(db, schema, table)
    values = info.model_dump(exclude={"columns", "primary_key"})
    values.update(
        name=table if name is None else name, attributes=info.columns, id_column=info.primary_key
    )
    values.update(overrides)
    layer = Layer.model_validate(values)
    if "style" not in overrides:
        layer.style = default_style(layer.geometry_type)
    if "extent" not in overrides:
        layer.extent = await table_extent(db, layer.table_info)
    return layer
