use atlas::{
    catalog::{Catalog, Publication},
    config::{Config, Limits},
    domain::{Dataset, Error, Principal, Query},
    exports::geojson::Export,
    migrations::migrate,
    postgis::{connection::Database, introspection::inspect},
    query::predicate::Predicate,
    services::publications::Publications,
};

/// Stores `publication` on first use, then opens it through `Publications::export`.
async fn open(
    db: &Database,
    publication: &Publication,
    query: &Query,
    user: Option<Predicate>,
    limits: &Limits,
) -> Result<Export, Error> {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").unwrap()),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec![publication.config.source.schema.clone()];
    config.limits = limits.clone();
    let id = &publication.config.id;
    let catalog = Catalog::new(db.clone(), config.clone()).unwrap();
    if catalog.get(id).await.is_err() {
        catalog
            .create(&publication.config, publication.metadata.clone())
            .await
            .unwrap();
    }
    Publications::new(db.clone(), config)
        .unwrap()
        .export(id, &Principal::default(), query, user.as_ref())
        .await
}

// R11
#[tokio::test]
async fn cursor_budgets_snapshot_and_cleanup() {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").unwrap()),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["export_test".into()];
    config.limits.pool_max = 2;
    config.limits.data_operations = 1;
    config.limits.export_operations = 1;
    config.limits.default_limit = 1;
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS export_test CASCADE",
        "CREATE SCHEMA export_test",
        "CREATE TABLE export_test.data(id bigint PRIMARY KEY, amount numeric, geom geometry(Point,3857))",
        "INSERT INTO export_test.data SELECT n, 12345678901234567890.123, ST_Transform(ST_SetSRID(ST_Point(-103,20),4326),3857) FROM generate_series(1,300) n",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let dataset: Dataset = serde_json::from_str(r#"{"id":"geojson_export","title":"Export","source":{"schema":"export_test","table":"data"},"properties":["amount"],"access":{"public":true,"public_download":true}}"#).unwrap();
    let metadata = inspect(&db, &config, &dataset).await.unwrap();
    let publication = Publication {
        config: dataset,
        metadata,
        revision: 1,
    };
    let mut export = open(&db, &publication, &Query::default(), None, &config.limits)
        .await
        .unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("DELETE FROM export_test.data WHERE id > 200")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut text = String::new();
    while let Some(chunk) = export.next().await.unwrap() {
        text.push_str(&chunk);
    }
    assert!(text.contains("12345678901234567890.123"));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["features"].as_array().unwrap().len(), 300);
    assert_eq!(json["features"][0]["id"], 1);
    assert_eq!(json["features"][0]["geometry"]["coordinates"][0], -103);
    for query in [
        Query::default(),
        serde_json::from_str(r#"{"filter":{"op":"eq","field":"amount","value":0}}"#).unwrap(),
    ] {
        let user = query
            .filter
            .as_ref()
            .map(|f| atlas::query::predicate::parse(f.get(), &config.limits).unwrap());
        let mut baseline = open(&db, &publication, &query, user.clone(), &config.limits)
            .await
            .unwrap();
        let mut expected = String::new();
        while let Some(chunk) = baseline.next().await.unwrap() {
            expected.push_str(&chunk);
        }
        for budget in [expected.len(), expected.len() - 1] {
            let mut limits = config.limits.clone();
            limits.export_bytes = budget as u32;
            let mut export = open(&db, &publication, &query, user.clone(), &limits)
                .await
                .unwrap();
            let mut emitted = String::new();
            loop {
                match export.next().await {
                    Ok(Some(chunk)) => emitted.push_str(&chunk),
                    Ok(None) => {
                        assert_eq!(emitted, expected);
                        break;
                    }
                    Err(error) => {
                        assert_eq!(error, Error::QueryTooLarge);
                        assert_eq!(budget + 1, expected.len());
                        assert!(!emitted.ends_with("]}"));
                        assert!(emitted.len() + 2 <= budget);
                        break;
                    }
                }
            }
            assert!(emitted.len() <= budget);
        }
    }
    config.limits.export_features = 10;
    assert_eq!(
        open(&db, &publication, &Query::default(), None, &config.limits)
            .await
            .err(),
        Some(Error::QueryTooLarge)
    );
    config.limits.export_features = 1000;
    config.limits.export_bytes = 50;
    let mut export = open(&db, &publication, &Query::default(), None, &config.limits)
        .await
        .unwrap();
    export.next().await.unwrap();
    assert_eq!(export.next().await.err(), Some(Error::QueryTooLarge));
    drop(export);
    for _ in 0..8 {
        let export = open(&db, &publication, &Query::default(), None, &config.limits)
            .await
            .unwrap();
        drop(export);
    }
    // `stream_seconds` is applied by the HTTP stream: see api_export.rs.
    let query: Query = serde_json::from_str(r#"{"limit":1}"#).unwrap();
    assert_eq!(
        open(&db, &publication, &query, None, &config.limits)
            .await
            .err(),
        Some(Error::InvalidRequest)
    );
    db.close().await;
}

const HEAD: &str = r#"{"type":"FeatureCollection","features":["#;

async fn drain(export: &mut Export) -> (String, Option<Error>) {
    let mut text = String::new();
    loop {
        match export.next().await {
            Ok(Some(chunk)) => text.push_str(&chunk),
            Ok(None) => return (text, None),
            Err(error) => return (text, Some(error)),
        }
    }
}

async fn publication(db: &Database, config: &Config, table: &str, rows: &str) -> Publication {
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        format!("DROP TABLE IF EXISTS export_batch.{table}"),
        format!(
            "CREATE TABLE export_batch.{table}(id bigint PRIMARY KEY, label text, geom geometry(Point,4326))"
        ),
        format!("INSERT INTO export_batch.{table} {rows}"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let dataset: Dataset = serde_json::from_str(&format!(
        r#"{{"id":"geojson_{table}","title":"Batch","source":{{"schema":"export_batch","table":"{table}"}},"properties":["label"],"access":{{"public":true,"public_download":true}}}}"#
    ))
    .unwrap();
    let metadata = inspect(db, config, &dataset).await.unwrap();
    Publication {
        config: dataset,
        metadata,
        revision: 1,
    }
}

#[tokio::test]
async fn cursor_cuts_rows_one_by_one_within_budget() {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").unwrap()),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["export_batch".into()];
    config.limits.pool_max = 2;
    config.limits.data_operations = 1;
    config.limits.export_operations = 1;
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS export_batch CASCADE",
        "CREATE SCHEMA export_batch",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let all = Query::default();
    // Five equally sized ~200 B features: one-digit ids, same label and point.
    let small = publication(
        &db,
        &config,
        "small",
        "SELECT n, repeat('s', 150), ST_SetSRID(ST_Point(1,1),4326) FROM generate_series(1,5) n",
    )
    .await;
    let (baseline, error) =
        drain(&mut open(&db, &small, &all, None, &config.limits).await.unwrap()).await;
    assert_eq!(error, None);
    let size = (baseline.len() - HEAD.len() - 2 - 4) / 5;
    assert_eq!(HEAD.len() + 5 * size + 4 + 2, baseline.len());
    let two = HEAD.len() + 2 * size + 1;
    let mut limits = config.limits.clone();
    limits.export_bytes = (two + 2) as u32;
    let (text, error) = drain(&mut open(&db, &small, &all, None, &limits).await.unwrap()).await;
    assert_eq!(error, Some(Error::QueryTooLarge));
    assert_eq!(text, &baseline[..two]);
    assert!(!text.ends_with("]}"));
    // About 1 MB in one feature against a 64 KiB budget: nothing of it is emitted.
    let big = publication(
        &db,
        &config,
        "big",
        "SELECT 1, repeat('b', 1000000), ST_SetSRID(ST_Point(1,1),4326)",
    )
    .await;
    limits.export_bytes = 65536;
    let (text, error) = drain(&mut open(&db, &big, &all, None, &limits).await.unwrap()).await;
    assert_eq!(error, Some(Error::QueryTooLarge));
    assert_eq!(text, HEAD);
    // Within-budget exports after both cuts are unchanged.
    let (text, error) = drain(&mut open(&db, &small, &all, None, &limits).await.unwrap()).await;
    assert_eq!((text, error), (baseline, None));
    db.close().await;
}
