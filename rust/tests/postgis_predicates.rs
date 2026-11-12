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
    table: &str,
    fixed: Option<&Predicate>,
    user: Option<&Predicate>,
    meta: &Metadata,
) -> Result<QueryBuilder<Postgres>, Error> {
    let mut builder = QueryBuilder::new(format!("SELECT id FROM predicates_test.{table} WHERE "));
    restrict(&mut builder, fixed, user, meta)?;
    builder.push(" ORDER BY id");
    Ok(builder)
}

fn compile(table: &str, json: &str, meta: &Metadata) -> Result<QueryBuilder<Postgres>, Error> {
    select(table, None, Some(&filter(json)), meta)
}

async fn ids(db: &Database, mut builder: QueryBuilder<Postgres>) -> Result<Vec<i32>, Error> {
    let mut tx = db.begin_read().await?;
    builder
        .build_query_scalar::<i32>()
        .fetch_all(&mut *tx)
        .await
        .map_err(query_error)
}

// Filter rejection may happen while compiling or when PostgreSQL casts the bound text.
async fn refused(db: &Database, table: &str, json: &str, meta: &Metadata) -> bool {
    match compile(table, json, meta) {
        Err(error) => error == Error::InvalidFilter,
        Ok(builder) => ids(db, builder).await == Err(Error::InvalidFilter),
    }
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
        "DROP SCHEMA IF EXISTS predicates_test CASCADE",
        "CREATE SCHEMA predicates_test",
        "CREATE TABLE predicates_test.places(id integer PRIMARY KEY, \"we\"\"ird\" text, small int2, big int8, amount numeric, ratio float8, active bool, day date, seen timestamptz, category text, secret text, geom geometry(Point,4326))",
        "INSERT INTO predicates_test.places VALUES (1,'Alpha',1,9223372036854775807,12345678901234567890.123,0.5,true,'2026-01-01','2026-01-01T10:00:00Z','a','x',NULL), (2,'beta',2,0,1.5,1.5,false,'2026-06-01','2026-06-01T00:00:00+02:00','a','x',NULL), (3,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,'b','x',NULL), (4,'Gamma',4,-5,10,2.0,true,'2025-12-31','2025-12-31T23:59:59Z','b','x',NULL)",
        "CREATE TABLE predicates_test.edges(id integer PRIMARY KEY, amount numeric, ratio float8, single float4, whole int4, day date, stamp timestamp, seen timestamptz, label text, geom geometry(Point,4326))",
        "INSERT INTO predicates_test.edges VALUES (1,123456789012345678901234567890.5,1,1,1,'2024-01-01','2024-01-01T10:00:00','2024-01-01 08:00:00+00','x',NULL), (2,0,2,2,2,'2024-02-29','2024-01-01T12:00:00','2024-01-01 10:00:00+00','y',NULL), (3,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL)",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    config.allowed_schemas = vec!["predicates_test".into()];
    let dataset: Dataset = serde_json::from_str(r#"{"id":"places","title":"Places","source":{"schema":"predicates_test","table":"places"},"properties":["we\"ird","small","big","amount","ratio","active","day","seen","category"]}"#).unwrap();
    let meta = inspect(&db, &config, &dataset).await.unwrap();
    let dataset: Dataset = serde_json::from_str(r#"{"id":"edges","title":"Edges","source":{"schema":"predicates_test","table":"edges"},"properties":["amount","ratio","single","whole","day","stamp","seen","label"]}"#).unwrap();
    let edges = inspect(&db, &config, &dataset).await.unwrap();
    (db, meta, edges)
}

#[tokio::test]
async fn attribute_filters_against_real_postgis() {
    let (db, meta, edges) = setup().await;
    expected_rows(&db, &meta).await;
    lossless_numeric_and_bounded_floats(&db, &edges).await;
    strict_temporal_literals(&db, &edges).await;
    long_strings_are_bounded_only_by_filter_bytes(&db, &edges).await;
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
        let builder = compile("places", json, meta).unwrap();
        assert!(
            !builder.sql().as_str().contains("Alpha") && !builder.sql().as_str().contains("2026")
        );
        assert_eq!(ids(db, builder).await.unwrap(), expected, "{json}");
    }
    let everything = select("places", None, None, meta).unwrap();
    assert!(everything.sql().as_str().contains("WHERE TRUE"));
    assert_eq!(ids(db, everything).await.unwrap(), vec![1, 2, 3, 4]);
    assert!(
        refused(
            db,
            "places",
            r#"{"op":"eq","field":"day","value":"2026-02-30"}"#,
            meta
        )
        .await
    );
}

async fn lossless_numeric_and_bounded_floats(db: &Database, edges: &Metadata) {
    for (json, expected) in [
        (
            r#"{"op":"eq","field":"amount","value":123456789012345678901234567890.5}"#,
            vec![1],
        ),
        (
            r#"{"op":"eq","field":"amount","value":123456789012345678901234567890.4}"#,
            vec![],
        ),
        (
            r#"{"op":"ne","field":"amount","value":123456789012345678901234567890.4}"#,
            vec![1, 2],
        ),
        (
            r#"{"op":"in","field":"amount","value":[123456789012345678901234567890.5,7]}"#,
            vec![1],
        ),
        (r#"{"op":"gt","field":"amount","value":1e400}"#, vec![]),
        (r#"{"op":"lt","field":"amount","value":1e400}"#, vec![1, 2]),
        (r#"{"op":"gt","field":"amount","value":-1e400}"#, vec![1, 2]),
        // 1e-400 underflows to 0 as f64; exact comparison keeps it above the zero row.
        (r#"{"op":"lt","field":"amount","value":1e-400}"#, vec![2]),
        (r#"{"op":"eq","field":"amount","value":1e-400}"#, vec![]),
        (r#"{"op":"gt","field":"ratio","value":1}"#, vec![2]),
        (r#"{"op":"lt","field":"single","value":1e38}"#, vec![1, 2]),
        (r#"{"op":"eq","field":"whole","value":2147483647}"#, vec![]),
    ] {
        let builder = compile("edges", json, edges).unwrap();
        assert_eq!(ids(db, builder).await.unwrap(), expected, "{json}");
    }
    for json in [
        r#"{"op":"eq","field":"ratio","value":1e400}"#,
        r#"{"op":"gt","field":"ratio","value":-1e400}"#,
        r#"{"op":"in","field":"ratio","value":[1,1e400]}"#,
        r#"{"op":"lt","field":"single","value":1e39}"#,
        r#"{"op":"eq","field":"whole","value":1e400}"#,
        r#"{"op":"eq","field":"whole","value":2147483648}"#,
        r#"{"op":"eq","field":"whole","value":1.5}"#,
        r#"{"op":"eq","field":"amount","value":"1"}"#,
    ] {
        assert!(
            matches!(compile("edges", json, edges), Err(Error::InvalidFilter)),
            "{json}"
        );
    }
}

async fn strict_temporal_literals(db: &Database, edges: &Metadata) {
    for (json, expected) in [
        (r#"{"op":"eq","field":"day","value":"2024-01-01"}"#, vec![1]),
        (r#"{"op":"eq","field":"day","value":"2024-02-29"}"#, vec![2]),
        (
            r#"{"op":"eq","field":"stamp","value":"2024-01-01T10:00:00"}"#,
            vec![1],
        ),
        (
            r#"{"op":"gt","field":"stamp","value":"2024-01-01T10:00:00.5"}"#,
            vec![2],
        ),
        (
            r#"{"op":"eq","field":"seen","value":"2024-01-01T10:00:00+02:00"}"#,
            vec![1],
        ),
        (
            r#"{"op":"eq","field":"seen","value":"2024-01-01T08:00:00"}"#,
            vec![1],
        ),
        (
            r#"{"op":"eq","field":"seen","value":"2024-01-01T08:00:00Z"}"#,
            vec![1],
        ),
        (
            r#"{"op":"eq","field":"seen","value":"2024-01-01T05:00:00-05:00"}"#,
            vec![2],
        ),
    ] {
        let builder = compile("edges", json, edges).unwrap();
        assert_eq!(ids(db, builder).await.unwrap(), expected, "{json}");
    }
    for json in [
        r#"{"op":"eq","field":"day","value":"2024-01-01T00:00:00+05:00"}"#,
        r#"{"op":"eq","field":"day","value":"2024-01-01T00:00:00"}"#,
        r#"{"op":"eq","field":"day","value":"2024-01-01Z"}"#,
        r#"{"op":"eq","field":"day","value":"2024-02-30"}"#,
        r#"{"op":"eq","field":"day","value":"2023-02-29"}"#,
        r#"{"op":"eq","field":"day","value":"2024-13-01"}"#,
        r#"{"op":"eq","field":"day","value":"2024-1-01"}"#,
        r#"{"op":"in","field":"day","value":["2024-01-01","2024-02-30"]}"#,
        r#"{"op":"eq","field":"stamp","value":"2024-01-01T10:00:00Z"}"#,
        r#"{"op":"eq","field":"stamp","value":"2024-01-01T10:00:00+02:00"}"#,
        r#"{"op":"eq","field":"stamp","value":"2024-01-01T10:00:00-03:00"}"#,
        r#"{"op":"eq","field":"stamp","value":"2024-01-01T10:00:00.5+02:00"}"#,
        r#"{"op":"in","field":"stamp","value":["2024-01-01T10:00:00","2024-01-01T10:00:00Z"]}"#,
        r#"{"op":"eq","field":"seen","value":"2024-02-30T00:00:00Z"}"#,
    ] {
        assert!(refused(db, "edges", json, edges).await, "{json}");
    }
}

async fn long_strings_are_bounded_only_by_filter_bytes(db: &Database, edges: &Metadata) {
    let long = "x".repeat(60 * 1024);
    let json = format!(r#"{{"op":"eq","field":"label","value":"{long}"}}"#);
    assert!(json.len() <= Limits::default().filter_bytes as usize);
    let builder = compile("edges", &json, edges).unwrap();
    assert!(!builder.sql().as_str().contains(&long));
    assert_eq!(ids(db, builder).await.unwrap(), Vec::<i32>::new());
    let prefix = format!(r#"{{"op":"like","field":"label","value":"{long}%"}}"#);
    assert_eq!(
        ids(db, compile("edges", &prefix, edges).unwrap())
            .await
            .unwrap(),
        Vec::<i32>::new()
    );
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
        let builder = select("places", Some(&fixed), Some(&filter(json)), meta).unwrap();
        assert_eq!(ids(db, builder).await.unwrap(), expected, "{json}");
    }
    let only_fixed = select("places", Some(&fixed), None, meta).unwrap();
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
            matches!(compile("places", json, meta), Err(Error::InvalidFilter)),
            "{json}"
        );
    }
}
