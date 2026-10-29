use atlas::{
    config::{Config, Limits},
    domain::{Dataset, Error, Metadata},
    postgis::{
        connection::Database,
        introspection::inspect,
        predicates::{query_error, restrict},
    },
    query::predicate::{Predicate, parse},
};
use sqlx::{Postgres, QueryBuilder};

fn filter(json: &str) -> Predicate {
    parse(json, &Limits::default()).unwrap()
}

fn select(
    fixed: Option<&Predicate>,
    user: Option<&Predicate>,
    meta: &Metadata,
) -> Result<QueryBuilder<Postgres>, Error> {
    let mut builder = QueryBuilder::new("SELECT id FROM predicates_test.places WHERE ");
    restrict(&mut builder, fixed, user, meta)?;
    builder.push(" ORDER BY id");
    Ok(builder)
}

fn compile(json: &str, meta: &Metadata) -> Result<QueryBuilder<Postgres>, Error> {
    select(None, Some(&filter(json)), meta)
}

async fn ids(db: &Database, mut builder: QueryBuilder<Postgres>) -> Result<Vec<i32>, Error> {
    let mut tx = db.begin_read().await?;
    builder
        .build_query_scalar::<i32>()
        .fetch_all(&mut *tx)
        .await
        .map_err(query_error)
}

async fn setup() -> (Database, Metadata) {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS predicates_test CASCADE",
        "CREATE SCHEMA predicates_test",
        "CREATE TABLE predicates_test.places(id integer PRIMARY KEY, \"we\"\"ird\" text, small int2, big int8, amount numeric, ratio float8, active bool, day date, seen timestamptz, category text, secret text, geom geometry(Point,4326))",
        "INSERT INTO predicates_test.places VALUES (1,'Alpha',1,9223372036854775807,12345678901234567890.123,0.5,true,'2026-01-01','2026-01-01T10:00:00Z','a','x',NULL), (2,'beta',2,0,1.5,1.5,false,'2026-06-01','2026-06-01T00:00:00+02:00','a','x',NULL), (3,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,'b','x',NULL), (4,'Gamma',4,-5,10,2.0,true,'2025-12-31','2025-12-31T23:59:59Z','b','x',NULL)",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    config.allowed_schemas = vec!["predicates_test".into()];
    let dataset: Dataset = serde_json::from_str(r#"{"id":"places","title":"Places","source":{"schema":"predicates_test","table":"places"},"properties":["we\"ird","small","big","amount","ratio","active","day","seen","category"]}"#).unwrap();
    let meta = inspect(&db, &config, &dataset).await.unwrap();
    (db, meta)
}

#[tokio::test]
async fn attribute_filters_against_real_postgis() {
    let (db, meta) = setup().await;
    expected_rows(&db, &meta).await;
    fixed_filter_cannot_be_widened(&db, &meta).await;
    offensive_fields_operators_and_values_are_rejected(&meta);
}

async fn expected_rows(db: &Database, meta: &Metadata) {
    for (json, expected) in [
        (
            r#"{"op":"eq","field":"big","value":9223372036854775807}"#,
            vec![1],
        ),
        (
            r#"{"op":"eq","field":"amount","value":12345678901234567890.123}"#,
            vec![1],
        ),
        (
            r#"{"op":"eq","field":"amount","value":12345678901234567890.124}"#,
            vec![],
        ),
        (r#"{"op":"ne","field":"small","value":1}"#, vec![2, 4]),
        (
            r#"{"op":"not","arg":{"op":"eq","field":"small","value":1}}"#,
            vec![2, 4],
        ),
        (r#"{"op":"is_null","field":"small"}"#, vec![3]),
        (r#"{"op":"is_not_null","field":"we\"ird"}"#, vec![1, 2, 4]),
        (
            r#"{"op":"in","field":"category","value":["b","z"]}"#,
            vec![3, 4],
        ),
        (r#"{"op":"like","field":"we\"ird","value":"A%"}"#, vec![1]),
        (
            r#"{"op":"ilike","field":"we\"ird","value":"%A"}"#,
            vec![1, 2, 4],
        ),
        (
            r#"{"op":"gte","field":"day","value":"2026-01-01"}"#,
            vec![1, 2],
        ),
        (
            r#"{"op":"lt","field":"seen","value":"2026-03-01T00:00:00Z"}"#,
            vec![1, 4],
        ),
        (r#"{"op":"eq","field":"active","value":true}"#, vec![1, 4]),
        (r#"{"op":"gt","field":"ratio","value":1}"#, vec![2, 4]),
        (
            r#"{"op":"or","args":[{"op":"lte","field":"small","value":1},{"op":"eq","field":"category","value":"b"}]}"#,
            vec![1, 3, 4],
        ),
    ] {
        let builder = compile(json, meta).unwrap();
        assert!(
            !builder.sql().as_str().contains("Alpha") && !builder.sql().as_str().contains("2026")
        );
        assert_eq!(ids(db, builder).await.unwrap(), expected, "{json}");
    }
    let everything = select(None, None, meta).unwrap();
    assert!(everything.sql().as_str().contains("WHERE TRUE"));
    assert_eq!(ids(db, everything).await.unwrap(), vec![1, 2, 3, 4]);
    let invalid_day = compile(r#"{"op":"eq","field":"day","value":"2026-02-30"}"#, meta).unwrap();
    assert_eq!(ids(db, invalid_day).await, Err(Error::InvalidFilter));
}

async fn fixed_filter_cannot_be_widened(db: &Database, meta: &Metadata) {
    let fixed = filter(r#"{"op":"eq","field":"category","value":"a"}"#);
    for (json, expected) in [
        (
            r#"{"op":"or","args":[{"op":"eq","field":"category","value":"b"},{"op":"lte","field":"small","value":1}]}"#,
            vec![1],
        ),
        (
            r#"{"op":"not","arg":{"op":"eq","field":"category","value":"a"}}"#,
            vec![],
        ),
        (r#"{"op":"is_not_null","field":"category"}"#, vec![1, 2]),
    ] {
        let builder = select(Some(&fixed), Some(&filter(json)), meta).unwrap();
        assert_eq!(ids(db, builder).await.unwrap(), expected, "{json}");
    }
    let only_fixed = select(Some(&fixed), None, meta).unwrap();
    assert_eq!(ids(db, only_fixed).await.unwrap(), vec![1, 2]);
}

fn offensive_fields_operators_and_values_are_rejected(meta: &Metadata) {
    for json in [
        r#"{"op":"eq","field":"secret","value":"x"}"#,
        r#"{"op":"eq","field":"id","value":1}"#,
        r#"{"op":"is_null","field":"geom"}"#,
        r#"{"op":"eq","field":"x\"; DROP TABLE predicates_test.places; --","value":1}"#,
        r#"{"op":"like","field":"small","value":"1%"}"#,
        r#"{"op":"eq","field":"small","value":"1"}"#,
        r#"{"op":"eq","field":"small","value":1.5}"#,
        r#"{"op":"eq","field":"small","value":40000}"#,
        r#"{"op":"eq","field":"big","value":9223372036854775808}"#,
        r#"{"op":"eq","field":"ratio","value":"0.5"}"#,
        r#"{"op":"eq","field":"category","value":true}"#,
        r#"{"op":"eq","field":"active","value":1}"#,
        r#"{"op":"eq","field":"day","value":"now"}"#,
        r#"{"op":"eq","field":"day","value":"Jan 1 2026"}"#,
        r#"{"op":"in","field":"small","value":[1,"2"]}"#,
        r#"{"op":"and","args":[{"op":"is_null","field":"small"},{"op":"is_null","field":"secret"}]}"#,
        r#"{"op":"intersects","geometry":{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}}"#,
    ] {
        assert!(
            matches!(compile(json, meta), Err(Error::InvalidFilter)),
            "{json}"
        );
    }
}
