"""Publishable layer metadata and the services enabled for it."""

from typing import Self

from pydantic import BaseModel, Field, model_validator

from atlas.postgis.introspection import BBox, ColumnInfo, GeometryType, TableInfo


class LayerServices(BaseModel):
    wms: bool = True
    wfs: bool = True
    wmts: bool = False
    wcs: bool = False


class Layer(BaseModel):
    name: str = Field(pattern=r"^[a-z0-9_-]+$")
    title: str = ""
    description: str | None = None
    schema_name: str
    table_name: str
    geometry_column: str
    geometry_type: GeometryType
    srid: int
    id_column: str | None = None
    is_view: bool = False
    attributes: list[ColumnInfo] = Field(default_factory=list)
    extent: BBox | None = None
    enabled: bool = True
    searchable: bool = True
    downloadable: bool = True
    public: bool = True
    services: LayerServices = Field(default_factory=LayerServices)
    # shortcut: styles are unavailable until issue #28 supplies the validated Style models.
    style: None = None
    permissions: list[str] = Field(default_factory=list)
    min_zoom: int | None = None
    max_zoom: int | None = None

    @model_validator(mode="after")
    def _exclude_geometry(self) -> Self:
        if "title" not in self.model_fields_set:
            self.title = self.name.capitalize()
        self.attributes = [a for a in self.attributes if a.name != self.geometry_column]
        return self

    @property
    def attribute_names(self) -> list[str]:
        """Return the attribute whitelist, excluding geometry even after list mutation."""
        return [a.name for a in self.attributes if a.name != self.geometry_column]

    @property
    def table_info(self) -> TableInfo:
        return TableInfo(
            schema_name=self.schema_name,
            table_name=self.table_name,
            geometry_column=self.geometry_column,
            geometry_type=self.geometry_type,
            srid=self.srid,
            columns=[a for a in self.attributes if a.name != self.geometry_column],
            primary_key=self.id_column,
            is_view=self.is_view,
        )

    def supports(self, service: str) -> bool:
        return (
            self.enabled
            and service in LayerServices.model_fields
            and bool(getattr(self.services, service))
        )
