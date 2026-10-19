"""CSV BOM, quoting, null cells, geometry modes and lazy source consumption."""

import codecs
import csv
from io import StringIO

import pytest

from atlas.exports.csv_export import CSVExporter
from atlas.layers.models import Layer
from atlas.postgis.introspection import ColumnInfo


@pytest.fixture
def layer():
    return Layer(
        name="data",
        schema_name="geo",
        table_name="data",
        geometry_column="shape",
        geometry_type="Point",
        srid=4326,
        id_column="id",
        attributes=[
            ColumnInfo(name="id", data_type="int4", nullable=False),
            ColumnInfo(name="name", data_type="text", nullable=True),
        ],
    )


@pytest.mark.parametrize(
    "mode,columns,expected",
    [
        ("wkt", ["geometry"], ["POINT (1 2)"]),
        ("centroid", ["lon", "lat"], ["1.0", "2.0"]),
        ("none", [], []),
    ],
)
async def test_csv_escaping_bom_nulls_and_geometry(layer, mode, columns, expected):
    async def rows():
        yield {
            "id": 1,
            "name": 'México, "Norte"\nSur',
            "shape": {"type": "Point", "coordinates": [1, 2]},
        }
        yield {"id": 2, "name": None, "shape": None}

    body = b"".join([chunk async for chunk in CSVExporter(mode).export(rows(), layer)])
    assert body.startswith(codecs.BOM_UTF8)
    data = list(csv.reader(StringIO(body.decode("utf-8-sig"), newline="")))
    assert data[0] == ["id", "name", *columns]
    assert data[1] == ["1", 'México, "Norte"\nSur', *expected]
    assert data[2] == ["2", "", *([""] * len(columns))]


async def test_one_row_at_a_time(layer):
    consumed = []

    async def rows():
        for i in range(3):
            consumed.append(i)
            yield {"id": i, "name": f"row {i}"}

    stream = CSVExporter("none").export(rows(), layer)
    await anext(stream)
    assert consumed == []
    assert b"row 0" in await anext(stream)
    assert consumed == [0]
    assert b"row 0" not in await anext(stream)
    await stream.aclose()
    with pytest.raises(ValueError):
        CSVExporter("bad")
