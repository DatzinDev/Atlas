use atlas::{
    config::{Config, Limits},
    domain::{Dataset, Error, Metadata, Query},
    postgis::{
        connection::Database,
        features::page,
        introspection::inspect,
        mvt::{bounds, tile},
        selection::prepare,
    },
};
use std::collections::BTreeMap;

type Source = (Dataset, Metadata);

#[derive(Debug, PartialEq)]
struct Feature {
    id: Option<u64>,
    properties: BTreeMap<String, String>,
    geometry: Vec<u64>,
}

fn varint(buf: &[u8], pos: &mut usize) -> u64 {
    let (mut value, mut shift) = (0, 0);
    loop {
        let byte = buf[*pos];
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return value;
        }
        shift += 7;
    }
}

fn fields(buf: &[u8]) -> Vec<(u64, u64, &[u8])> {
    let (mut pos, mut out) = (0, Vec::new());
    while pos < buf.len() {
        let key = varint(buf, &mut pos);
        let (number, bytes) = match key & 7 {
            0 => (varint(buf, &mut pos), &buf[0..0]),
            1 | 5 => {
                let size = if key & 7 == 1 { 8 } else { 4 };
                pos += size;
                (0, &buf[pos - size..pos])
            }
            2 => {
                let size = varint(buf, &mut pos) as usize;
                pos += size;
                (0, &buf[pos - size..pos])
            }
            wire => panic!("unexpected wire type {wire}"),
        };
        out.push((key >> 3, number, bytes));
    }
    out
}

fn packed(buf: &[u8]) -> Vec<u64> {
    let (mut pos, mut out) = (0, Vec::new());
    while pos < buf.len() {
        out.push(varint(buf, &mut pos));
    }
    out
}

fn value(buf: &[u8]) -> String {
    let (field, number, bytes) = fields(buf)[0];
    match field {
        1 => String::from_utf8(bytes.to_vec()).unwrap(),
        2 => f32::from_le_bytes(bytes.try_into().unwrap()).to_string(),
        3 => f64::from_le_bytes(bytes.try_into().unwrap()).to_string(),
        4 => (number as i64).to_string(),
        6 => ((number >> 1) as i64 ^ -((number & 1) as i64)).to_string(),
        7 => (number != 0).to_string(),
        _ => number.to_string(),
    }
}

fn decode(tile: &[u8]) -> BTreeMap<String, (u64, Vec<Feature>)> {
    let mut layers = BTreeMap::new();
    for (_, _, layer) in fields(tile).into_iter().filter(|f| f.0 == 3) {
        let parts = fields(layer);
        let text = |n| parts.iter().filter(move |f| f.0 == n).map(|f| f.2);
        let name = String::from_utf8(text(1).next().unwrap().to_vec()).unwrap();
        let keys: Vec<_> = text(3)
            .map(|k| String::from_utf8(k.to_vec()).unwrap())
            .collect();
        let values: Vec<_> = text(4).map(value).collect();
        let extent = parts.iter().find(|f| f.0 == 5).map_or(4096, |f| f.1);
        let features = text(2)
            .map(|feature| {
                let parts = fields(feature);
                let tags = parts
                    .iter()
                    .find(|f| f.0 == 2)
                    .map_or(vec![], |f| packed(f.2));
                Feature {
                    id: parts.iter().find(|f| f.0 == 1).map(|f| f.1),
                    properties: tags
                        .chunks(2)
                        .map(|t| (keys[t[0] as usize].clone(), values[t[1] as usize].clone()))
                        .collect(),
                    geometry: parts
                        .iter()
                        .find(|f| f.0 == 4)
                        .map_or(vec![], |f| packed(f.2)),
                }
            })
            .collect();
        layers.insert(name, (extent, features));
    }
    layers
}

/// Absolute tile-space vertices of an MVT command stream (MoveTo/LineTo; ClosePath has none).
fn vertices(geometry: &[u64]) -> Vec<(i64, i64)> {
    let zigzag = |n: u64| (n >> 1) as i64 ^ -((n & 1) as i64);
    let (mut out, mut cursor, mut i) = (Vec::new(), (0, 0), 0);
    while i < geometry.len() {
        let (command, count) = (geometry[i] & 7, (geometry[i] >> 3) as usize);
        i += 1;
        if command == 7 {
            continue;
        }
        assert!(command == 1 || command == 2, "unexpected command {command}");
        for _ in 0..count {
            cursor = (
                cursor.0 + zigzag(geometry[i]),
                cursor.1 + zigzag(geometry[i + 1]),
            );
            out.push(cursor);
            i += 2;
        }
    }
    out
}

fn tile_of(lon: f64, lat: f64, z: u32) -> (u32, u32, u32) {
    let n = f64::from(1u32 << z);
    let y = (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n;
    (z, ((lon + 180.0) / 360.0 * n) as u32, y as u32)
}

async fn fetch(
    db: &Database,
    (dataset, meta): &Source,
    xyz: (u32, u32, u32),
    filter: Option<&str>,
    properties: Option<&[String]>,
    limits: &Limits,
) -> Result<Vec<u8>, Error> {
    let mut tx = db.begin_read().await?;
    let selection = prepare(&mut tx, None, filter, None, meta, limits).await?;
    tile(&mut tx, dataset, meta, &selection, xyz, properties, limits).await
}

async fn setup() -> (Database, [Source; 5]) {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["mvt_test".into()];
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS mvt_test CASCADE",
        "CREATE SCHEMA mvt_test",
        "CREATE TABLE mvt_test.points(id bigint PRIMARY KEY, name text, amount numeric, ratio float8, big int8, day date, at timestamptz, local timestamp, geom geometry(Point,4326))",
        "INSERT INTO mvt_test.points(id,name,amount,ratio,big,day,geom) VALUES (1,'a',12345678901234567890.123,'NaN',9223372036854775807,'2026-01-01',ST_SetSRID(ST_Point(10,10),4326)), (9007199254740993,'c',NULL,NULL,NULL,NULL,ST_SetSRID(ST_Point(10.2,10.2),4326)), (4,'east',NULL,NULL,NULL,NULL,ST_SetSRID(ST_Point(179.9,10),4326)), (5,'west',NULL,NULL,NULL,NULL,ST_SetSRID(ST_Point(-179.9,10),4326)), (6,'none',NULL,NULL,NULL,NULL,NULL), (9007199254740991,'max',NULL,NULL,NULL,NULL,ST_SetSRID(ST_Point(50,-50),4326)), (-1,'negative',NULL,NULL,NULL,NULL,ST_SetSRID(ST_Point(50,-50),4326))",
        "INSERT INTO mvt_test.points VALUES (2,'b',1.5,0.5,1,'2026-01-02','2024-05-01 12:00:00+00','2024-05-01 12:00:00',ST_SetSRID(ST_Point(10.1,10.1),4326))",
        "CREATE TABLE mvt_test.utm(id integer PRIMARY KEY, name text, geom geometry(Point,32613))",
        "INSERT INTO mvt_test.utm VALUES (1,'gdl',ST_Transform(ST_SetSRID(ST_Point(-103.35,20.67),4326),32613))",
        "CREATE TABLE mvt_test.areas(id integer PRIMARY KEY, name text, geom geometry(Polygon,3857))",
        "INSERT INTO mvt_test.areas VALUES (1,'cross',ST_Transform(ST_MakeEnvelope(-1,-1,1,1,4326),3857))",
        "CREATE TABLE mvt_test.grid(id integer PRIMARY KEY, name text, geom geometry(Point,32614))",
        // 0.05 degree mesh covering the buffered tile 6/14/28; offset so a row (lat 16.56) falls in
        // the curved bottom band an undensified envelope misses.
        "INSERT INTO mvt_test.grid SELECT i * 1000 + j, 'p', ST_Transform(ST_SetSRID(ST_Point((-102 + i * 0.05)::float8, (16.01 + j * 0.05)::float8), 4326), 32614) FROM generate_series(0, 140) AS i, generate_series(0, 130) AS j",
        "CREATE TABLE mvt_test.codes(code text PRIMARY KEY, name text, geom geometry(Point,4326))",
        "INSERT INTO mvt_test.codes VALUES ('5','five',ST_SetSRID(ST_Point(10,10),4326))",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let mut sources = Vec::new();
    for (table, properties) in [
        (
            "points",
            vec!["name", "amount", "ratio", "big", "day", "at", "local"],
        ),
        ("utm", vec!["name"]),
        ("areas", vec!["name"]),
        ("grid", vec!["name"]),
        ("codes", vec!["name"]),
    ] {
        let dataset: Dataset =
            serde_json::from_value(serde_json::json!({"id": table, "title": table,
            "source": {"schema": "mvt_test", "table": table}, "properties": properties}))
            .unwrap();
        let meta = inspect(&db, &config, &dataset).await.unwrap();
        sources.push((dataset, meta));
    }
    (db, sources.try_into().ok().unwrap())
}

#[tokio::test]
async fn native_mvt_against_real_postgis() {
    let (db, [points, utm, areas, grid, codes]) = setup().await;
    let limits = Limits::default();
    let here = tile_of(10.1, 10.1, 4);
    let encoded = fetch(&db, &points, here, None, None, &limits)
        .await
        .unwrap();
    let layers = decode(&encoded);
    let (extent, features) = &layers["points"];
    assert_eq!((layers.len(), *extent, features.len()), (1, 4096, 3));
    let first = features.iter().find(|f| f.id == Some(1)).unwrap();
    let expected: BTreeMap<String, String> = [
        ("name", "a"),
        ("amount", "12345678901234567890.123"),
        ("big", "9223372036854775807"),
        ("day", "2026-01-01"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    assert_eq!(first.properties, expected);
    let second = features.iter().find(|f| f.id == Some(2)).unwrap();
    assert_eq!(second.properties["ratio"], "0.5");
    assert!(
        features
            .iter()
            .any(|f| f.id.is_none() && f.properties["name"] == "c")
    );

    let name = ["name".to_owned()];
    let only = fetch(&db, &points, here, None, Some(&name), &limits)
        .await
        .unwrap();
    assert!(
        decode(&only)["points"]
            .1
            .iter()
            .all(|f| f.properties.len() == 1)
    );
    let b = Some(r#"{"op":"eq","field":"name","value":"b"}"#);
    let filtered = decode(&fetch(&db, &points, here, b, None, &limits).await.unwrap());
    assert_eq!(
        filtered["points"]
            .1
            .iter()
            .map(|f| f.id)
            .collect::<Vec<_>>(),
        [Some(2)]
    );
    let empty = fetch(&db, &points, tile_of(-100.0, -50.0, 4), None, None, &limits).await;
    assert_eq!(empty.unwrap(), Vec::<u8>::new());

    let names = |tile: Vec<u8>| -> Vec<String> {
        let layers = decode(&tile);
        layers.get("points").map_or(vec![], |l| {
            l.1.iter().map(|f| f.properties["name"].clone()).collect()
        })
    };
    let east = fetch(&db, &points, (1, 1, 0), None, Some(&name), &limits)
        .await
        .unwrap();
    assert!(names(east.clone()).contains(&"east".into()) && !names(east).contains(&"west".into()));
    let west = fetch(&db, &points, (1, 0, 0), None, Some(&name), &limits)
        .await
        .unwrap();
    assert!(names(west.clone()).contains(&"west".into()) && !names(west).contains(&"east".into()));
    for xyz in [(0, 0, 0), tile_of(-103.35, 20.67, 10)] {
        let local = decode(&fetch(&db, &utm, xyz, None, None, &limits).await.unwrap());
        assert_eq!(local["utm"].1[0].properties["name"], "gdl", "{xyz:?}");
    }
    for xyz in [(1, 0, 0), (1, 1, 0), (1, 0, 1), (1, 1, 1)] {
        let mosaic = decode(&fetch(&db, &areas, xyz, None, None, &limits).await.unwrap());
        assert_eq!(
            mosaic["areas"].1.iter().map(|f| f.id).collect::<Vec<_>>(),
            [Some(1)]
        );
    }

    let mut tight = Limits {
        tile_candidates: 3,
        ..Limits::default()
    };
    assert!(fetch(&db, &points, here, None, None, &tight).await.is_ok());
    tight.tile_candidates = 2;
    assert_eq!(
        fetch(&db, &points, here, None, None, &tight).await,
        Err(Error::QueryTooLarge)
    );
    tight = Limits {
        tile_bytes: encoded.len() as u32,
        ..Limits::default()
    };
    assert_eq!(
        fetch(&db, &points, here, None, None, &tight).await.unwrap(),
        encoded
    );
    tight.tile_bytes -= 1;
    assert_eq!(
        fetch(&db, &points, here, None, None, &tight).await,
        Err(Error::QueryTooLarge)
    );
    let unpublished = ["secret".to_owned()];
    let twice = ["name".to_owned(), "name".to_owned()];
    for (xyz, properties) in [
        ((23, 0, 0), None),
        ((4, 16, 0), None),
        (here, Some(&unpublished[..])),
        (here, Some(&twice[..])),
    ] {
        assert_eq!(
            fetch(&db, &points, xyz, None, properties, &limits).await,
            Err(Error::InvalidRequest)
        );
    }

    // The densified envelope returns exactly the UTM rows intersecting the buffered tile.
    let xyz = (6, 14, 28);
    let b = bounds(xyz.0, xyz.1, xyz.2, limits.tile_zoom).unwrap();
    let m = (b[2] - b[0]) * f64::from(limits.tile_buffer) / f64::from(limits.tile_extent);
    let mut tx = db.begin_read().await.unwrap();
    let (expected, missed): (Vec<i64>, i64) = sqlx::query_as(
        "SELECT array_agg(id::int8 ORDER BY id), count(*) FILTER (WHERE NOT geom && ST_Transform(e, 32614)) FROM mvt_test.grid, ST_MakeEnvelope($1, $2, $3, $4, 3857) AS e WHERE ST_Intersects(ST_Transform(geom, 3857), e)",
    )
    .bind(b[0] - m)
    .bind(b[1] - m)
    .bind(b[2] + m)
    .bind(b[3] + m)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    drop(tx);
    assert!(
        missed > 0,
        "the fixture must include rows the undensified envelope misses"
    );
    let encoded = fetch(&db, &grid, xyz, None, None, &limits).await.unwrap();
    let mut found: Vec<i64> = decode(&encoded)["grid"]
        .1
        .iter()
        .map(|f| f.id.unwrap() as i64)
        .collect();
    found.sort_unstable();
    assert_eq!(found, expected);

    // Temporal values are the GeoJSON ISO text; the zone is pinned for the exact offset.
    let mut tx = db.begin_read().await.unwrap();
    sqlx::query("SET LOCAL TimeZone = 'UTC'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let (dataset, meta) = &points;
    let selection = prepare(&mut tx, None, None, None, meta, &limits)
        .await
        .unwrap();
    let encoded = tile(&mut tx, dataset, meta, &selection, here, None, &limits)
        .await
        .unwrap();
    let query: Query = serde_json::from_str("{}").unwrap();
    let geojson = page(&mut tx, dataset, meta, &selection, &query, &limits)
        .await
        .unwrap();
    drop(tx);
    let feature = geojson
        .features
        .iter()
        .map(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).unwrap())
        .find(|f| f["id"] == 2)
        .unwrap();
    let layers = decode(&encoded);
    let mvt = &layers["points"].1.iter().find(|f| f.id == Some(2)).unwrap();
    for column in ["at", "local", "day"] {
        let text = feature["properties"][column].as_str().unwrap();
        assert_eq!(mvt.properties[column], text, "{column}");
    }
    assert_eq!(mvt.properties["at"], "2024-05-01T12:00:00+00:00");

    // Only integer ids in 0..=2^53-1 are encoded; a text id never is.
    let south = fetch(
        &db,
        &points,
        tile_of(50.0, -50.0, 4),
        None,
        Some(&name),
        &limits,
    )
    .await
    .unwrap();
    let mut ids: Vec<_> = decode(&south)["points"]
        .1
        .iter()
        .map(|f| (f.properties["name"].clone(), f.id))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            ("max".to_owned(), Some(9007199254740991)),
            ("negative".to_owned(), None)
        ]
    );
    let text = decode(&fetch(&db, &codes, here, None, None, &limits).await.unwrap());
    assert_eq!(text["codes"].1[0].id, None);
}

#[tokio::test]
async fn polar_vertices_are_clipped_to_the_mercator_domain() {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["mvt_polar".into()];
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS mvt_polar CASCADE",
        "CREATE SCHEMA mvt_polar",
        "CREATE TABLE mvt_polar.world(id integer PRIMARY KEY, name text, geom geometry(Geometry,4326))",
        "INSERT INTO mvt_polar.world VALUES (1,'antarctica',ST_GeomFromText('POLYGON((-180 -90,180 -90,180 -60,-180 -60,-180 -90))',4326)), (2,'pole',ST_GeomFromText('POINT(0 90)',4326)), (3,'mixed',ST_GeomFromText('MULTIPOINT((10 10),(0 90))',4326))",
        "CREATE TABLE mvt_polar.line(id integer PRIMARY KEY, name text, geom geometry(LineString,4326))",
        "INSERT INTO mvt_polar.line VALUES (1,'meridian',ST_GeomFromText('LINESTRING(0 0,0 90)',4326))",
        "CREATE TABLE mvt_polar.nad(id integer PRIMARY KEY, name text, geom geometry(Geometry,4269))",
        "INSERT INTO mvt_polar.nad VALUES (1,'antarctica',ST_GeomFromText('POLYGON((-180 -90,180 -90,180 -60,-180 -60,-180 -90))',4269)), (2,'pole',ST_GeomFromText('POINT(0 90)',4269))",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let mut sources = BTreeMap::new();
    for table in ["world", "line", "nad"] {
        let dataset: Dataset =
            serde_json::from_value(serde_json::json!({"id": table, "title": table,
            "source": {"schema": "mvt_polar", "table": table}, "properties": ["name"]}))
            .unwrap();
        let meta = inspect(&db, &config, &dataset).await.unwrap();
        sources.insert(table, (dataset, meta));
    }
    let limits = Limits::default();
    let ids = |tile: &[u8], layer: &str| -> Vec<u64> {
        let mut ids: Vec<u64> = decode(tile)
            .get(layer)
            .map_or(vec![], |l| l.1.iter().filter_map(|f| f.id).collect());
        ids.sort_unstable();
        ids
    };

    // Regression: a vertex at latitude +-90 made ST_Transform fail and the tile return 503.
    for (table, xyz) in [
        ("world", (0, 0, 0)),
        ("world", (1, 0, 1)),
        ("nad", (0, 0, 0)),
        ("nad", (1, 0, 1)),
    ] {
        let encoded = fetch(&db, &sources[table], xyz, None, None, &limits).await;
        let found = ids(
            &encoded.unwrap_or_else(|e| panic!("{table} {xyz:?}: {e:?}")),
            table,
        );
        assert!(
            found.contains(&1) && !found.contains(&2),
            "{table} {xyz:?}: {found:?}"
        );
    }
    // Only the representable part of a multi-geometry survives the clip.
    let world = fetch(&db, &sources["world"], (0, 0, 0), None, None, &limits)
        .await
        .unwrap();
    assert_eq!(ids(&world, "world"), [1, 3]);
    let mixed = decode(&world)["world"]
        .1
        .iter()
        .find(|f| f.id == Some(3))
        .map(|f| vertices(&f.geometry))
        .unwrap();
    assert_eq!(mixed.len(), 1, "{mixed:?}");
    // A point seen only beyond +-85.0511 is never encoded, not even as an error.
    let north = fetch(&db, &sources["world"], (1, 0, 0), None, None, &limits)
        .await
        .unwrap();
    assert!(!ids(&north, "world").contains(&2));

    let (extent, buffer) = (i64::from(limits.tile_extent), i64::from(limits.tile_buffer));
    for xyz in [(0, 0, 0), (1, 0, 0), (1, 1, 0)] {
        let encoded = fetch(&db, &sources["line"], xyz, None, None, &limits)
            .await
            .unwrap_or_else(|e| panic!("{xyz:?}: {e:?}"));
        let layers = decode(&encoded);
        let line = &layers["line"].1;
        assert_eq!(
            line.iter().map(|f| f.id).collect::<Vec<_>>(),
            [Some(1)],
            "{xyz:?}"
        );
        let points = vertices(&line[0].geometry);
        assert!(points.len() >= 2, "{xyz:?}: {points:?}");
        assert!(
            points
                .iter()
                .all(|&(x, y)| (-buffer..=extent + buffer).contains(&x)
                    && (-buffer..=extent + buffer).contains(&y)),
            "{xyz:?}: {points:?}"
        );
    }
    let corner = fetch(&db, &sources["line"], (1, 0, 1), None, None, &limits).await;
    assert!(corner.is_ok(), "{corner:?}");
}

#[test]
fn tile_sql_does_not_inspect_the_id_type_per_row() {
    // ponytail: the SQL builder is private, so the generated text is checked at its source.
    assert!(!include_str!("../src/postgis/mvt.rs").contains("pg_typeof"));
}
