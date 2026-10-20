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


@pytest.mark.parametrize(
    "value",
    [
        "=1+1",
        "+SUM(1,1)",
        "-1+2",
        "@SUM(1,1)",
        "  =1+1",
        "\t=1+1",
        "\r=1+1",
        "\n=1+1",
        "＝1+1",
        "＋1+1",
        "－1+1",
        "＠SUM(1,1)",
        '=HYPERLINK("https://example.invalid","click"),extra',
    ],
)
async def test_csv_neutralizes_formulas_in_values_and_headers(layer, value):
    layer.attributes[1] = ColumnInfo(name=value, data_type="text", nullable=True)

    async def rows():
        yield {"id": -42, value: value}

    body = b"".join([chunk async for chunk in CSVExporter("none").export(rows(), layer)])
    parsed = list(csv.reader(StringIO(body.decode("utf-8-sig"), newline="")))
    assert parsed == [["id", "'" + value], ["-42", "'" + value]]


async def test_csv_keeps_ordinary_text_numbers_and_nulls(layer):
    async def rows():
        for value in ["text = harmless", "123", "'already text", None, -1.5]:
            yield {"id": -1, "name": value}

    body = b"".join([chunk async for chunk in CSVExporter("none").export(rows(), layer)])
    parsed = list(csv.reader(StringIO(body.decode("utf-8-sig"))))
    assert [row[1] for row in parsed[1:]] == ["text = harmless", "123", "'already text", "", "-1.5"]
