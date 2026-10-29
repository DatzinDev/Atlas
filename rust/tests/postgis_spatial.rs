use atlas::{
    config::{Config, Limits},
    domain::{Dataset, Error, Metadata},
    postgis::{
        connection::Database,
        introspection::inspect,
        predicates::{query_error, restrict},
        spatial::validate,
    },
    query::predicate::{Predicate, parse},
};
use sqlx::{Postgres, QueryBuilder};

const HOLED: &str = "[[[0,0],[2,0],[2,2],[0,2],[0,0]],[[0.75,0.75],[1.25,0.75],[1.25,1.25],[0.75,1.25],[0.75,0.75]]]";

fn spatial(op: &str, coordinates: &str) -> Predicate {
    let json =
        format!(r#"{{"op":"{op}","geometry":{{"type":"Polygon","coordinates":{coordinates}}}}}"#);
    parse(&json, &Limits::default()).unwrap()
}

fn select(
    prefix: &str,
    table: &str,
    user: Option<&Predicate>,
    bbox: Option<&[f64; 4]>,
    meta: &Metadata,
) -> Result<QueryBuilder<Postgres>, Error> {
    let mut builder = QueryBuilder::new(format!("{prefix} spatial_test.{table} WHERE "));
    restrict(&mut builder, None, user, bbox, meta)?;
    builder.push(" ORDER BY id");
    Ok(builder)
}

async fn ids(
    db: &Database,
    table: &str,
    user: Option<&Predicate>,
    bbox: Option<&[f64; 4]>,
    meta: &Metadata,
) -> Result<Vec<i32>, Error> {
    let mut tx = db.begin_read().await?;
    if let Some(predicate) = user {
        validate(&mut tx, predicate, meta).await?;
    }
    select("SELECT id FROM", table, user, bbox, meta)?
        .build_query_scalar::<i32>()
        .fetch_all(&mut *tx)
        .await
        .map_err(query_error)
}

async fn setup() -> (Database, Metadata, Metadata) {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS spatial_test CASCADE",
        "CREATE SCHEMA spatial_test",
        "CREATE TABLE spatial_test.points(id integer PRIMARY KEY, name text, geom geometry(Point,4326))",
        "INSERT INTO spatial_test.points VALUES (1,'inside',ST_SetSRID(ST_Point(0.5,0.5),4326)), (2,'hole',ST_SetSRID(ST_Point(1,1),4326)), (3,'edge',ST_SetSRID(ST_Point(2,1),4326)), (4,'outside',ST_SetSRID(ST_Point(5,5),4326))",
        "INSERT INTO spatial_test.points SELECT n, 'far', ST_SetSRID(ST_Point(10 + n % 100, 10 + n / 100), 4326) FROM generate_series(100, 5100) AS n",
        "CREATE INDEX points_geom_idx ON spatial_test.points USING gist(geom)",
        "CREATE TABLE spatial_test.mercator(id integer PRIMARY KEY, name text, geom geometry(Point,3857))",
        "INSERT INTO spatial_test.mercator SELECT id, name, ST_Transform(geom, 3857) FROM spatial_test.points WHERE id IN (1, 2, 4)",
        "ANALYZE spatial_test.points",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    config.allowed_schemas = vec!["spatial_test".into()];
    let mut metas = Vec::new();
    for table in ["points", "mercator"] {
        let dataset: Dataset =
            serde_json::from_value(serde_json::json!({"id": table, "title": table,
            "source": {"schema": "spatial_test", "table": table}, "properties": ["name"]}))
            .unwrap();
        metas.push(inspect(&db, &config, &dataset).await.unwrap());
    }
    let mercator = metas.pop().unwrap();
    (db, metas.pop().unwrap(), mercator)
}

#[tokio::test]
async fn spatial_selections_against_real_postgis() {
    let (db, points, mercator) = setup().await;
    let intersects = spatial("intersects", HOLED);
    let within = spatial("within", HOLED);
    let bbox = [0.0, 0.0, 2.0, 2.0];
    for (table, meta, user, bbox, expected) in [
        ("points", &points, Some(&intersects), None, vec![1, 3]),
        ("points", &points, Some(&within), None, vec![1]),
        ("points", &points, None, Some(&bbox), vec![1, 2, 3]),
        ("points", &points, Some(&within), Some(&bbox), vec![1]),
        ("mercator", &mercator, Some(&intersects), None, vec![1]),
        ("mercator", &mercator, None, Some(&bbox), vec![1, 2]),
    ] {
        assert_eq!(
            ids(&db, table, user, bbox, meta).await.unwrap(),
            expected,
            "{table}"
        );
    }

    let mut tx = db.begin_read().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan: Vec<String> = select(
        "EXPLAIN SELECT id FROM",
        "points",
        Some(&intersects),
        Some(&bbox),
        &points,
    )
    .unwrap()
    .build_query_scalar()
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert!(plan.concat().contains("points_geom_idx"), "{plan:?}");
    invalid_or_out_of_domain_selections_fail_closed(&db, &points, &mercator).await;
}

async fn invalid_or_out_of_domain_selections_fail_closed(
    db: &Database,
    points: &Metadata,
    mercator: &Metadata,
) {
    for coordinates in [
        "[[[0,0],[2,2],[2,0],[0,2],[0,0]]]",
        "[[[0,0],[1,0],[2,0],[0,0]]]",
        "[[[0,0],[1,0],[1,1],[0,1],[0,0]],[[5,5],[6,5],[6,6],[5,6],[5,5]]]",
    ] {
        let predicate = spatial("intersects", coordinates);
        let result = ids(db, "points", Some(&predicate), None, points).await;
        assert_eq!(result, Err(Error::InvalidFilter), "{coordinates}");
    }
    let outside = spatial("within", "[[[0,85],[1,85],[1,95],[0,95],[0,85]]]");
    assert!(matches!(
        select("SELECT id FROM", "points", Some(&outside), None, points),
        Err(Error::InvalidFilter)
    ));
    let pole = spatial("within", "[[[0,85],[1,85],[1,90],[0,90],[0,85]]]");
    assert_eq!(
        ids(db, "mercator", Some(&pole), None, mercator).await,
        Err(Error::InvalidFilter)
    );
    for bbox in [[-181.0, 0.0, 1.0, 1.0], [0.0, -91.0, 1.0, 1.0]] {
        assert!(matches!(
            select("SELECT id FROM", "points", None, Some(&bbox), points),
            Err(Error::InvalidFilter)
        ));
    }
}
