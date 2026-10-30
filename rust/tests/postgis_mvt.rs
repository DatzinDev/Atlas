use atlas::{
    config::{Config, Limits},
    domain::{Dataset, Error, Metadata},
    postgis::{connection::Database, introspection::inspect, mvt::tile, selection::prepare},
};
use std::collections::BTreeMap;

type Source = (Dataset, Metadata);

#[derive(Debug, PartialEq)]
struct Feature {
    id: Option<u64>,
    properties: BTreeMap<String, String>,
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
                }
            })
            .collect();
        layers.insert(name, (extent, features));
    }
    layers
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

async fn setup() -> (Database, Source, Source, Source) {
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
        "CREATE TABLE mvt_test.points(id bigint PRIMARY KEY, name text, amount numeric, ratio float8, big int8, day date, geom geometry(Point,4326))",
        "INSERT INTO mvt_test.points VALUES (1,'a',12345678901234567890.123,'NaN',9223372036854775807,'2026-01-01',ST_SetSRID(ST_Point(10,10),4326)), (2,'b',1.5,0.5,1,'2026-01-02',ST_SetSRID(ST_Point(10.1,10.1),4326)), (9007199254740993,'c',NULL,NULL,NULL,NULL,ST_SetSRID(ST_Point(10.2,10.2),4326)), (4,'east',NULL,NULL,NULL,NULL,ST_SetSRID(ST_Point(179.9,10),4326)), (5,'west',NULL,NULL,NULL,NULL,ST_SetSRID(ST_Point(-179.9,10),4326)), (6,'none',NULL,NULL,NULL,NULL,NULL)",
        "CREATE TABLE mvt_test.utm(id integer PRIMARY KEY, name text, geom geometry(Point,32613))",
        "INSERT INTO mvt_test.utm VALUES (1,'gdl',ST_Transform(ST_SetSRID(ST_Point(-103.35,20.67),4326),32613))",
        "CREATE TABLE mvt_test.areas(id integer PRIMARY KEY, name text, geom geometry(Polygon,3857))",
        "INSERT INTO mvt_test.areas VALUES (1,'cross',ST_Transform(ST_MakeEnvelope(-1,-1,1,1,4326),3857))",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let mut sources = Vec::new();
    for (table, properties) in [
        ("points", vec!["name", "amount", "ratio", "big", "day"]),
        ("utm", vec!["name"]),
        ("areas", vec!["name"]),
    ] {
        let dataset: Dataset =
            serde_json::from_value(serde_json::json!({"id": table, "title": table,
            "source": {"schema": "mvt_test", "table": table}, "properties": properties}))
            .unwrap();
        let meta = inspect(&db, &config, &dataset).await.unwrap();
        sources.push((dataset, meta));
    }
    let areas = sources.pop().unwrap();
    let utm = sources.pop().unwrap();
    (db, sources.pop().unwrap(), utm, areas)
}

#[tokio::test]
async fn native_mvt_against_real_postgis() {
    let (db, points, utm, areas) = setup().await;
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
}
