use atlas::{
    config::{Config, Limits},
    domain::{Dataset, Error, Metadata},
    postgis::{connection::Database, introspection::inspect, mvt::tile, selection::prepare},
};

type Source = (Dataset, Metadata);

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
    let (db, points, _, _) = setup().await;
    let limits = Limits::default();
    let here = tile_of(10.1, 10.1, 4);
    let encoded = fetch(&db, &points, here, None, None, &limits)
        .await
        .unwrap();
    assert!(!encoded.is_empty());
    let empty = fetch(&db, &points, tile_of(-100.0, -50.0, 4), None, None, &limits).await;
    assert_eq!(empty.unwrap(), Vec::<u8>::new());
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
