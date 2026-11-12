use atlas::{
    catalog::Catalog,
    config::Config,
    domain::{Dataset, Error},
    migrations::migrate,
    postgis::{connection::Database, introspection::inspect},
};
use sqlx::types::Json;

const VERSIONS: &str =
    "SELECT version, applied_at::text FROM atlas_catalog.migrations ORDER BY version";
const DATASETS: &str = "SELECT id, config::text, metadata::text, revision, created_at::text, updated_at::text FROM atlas_catalog.datasets ORDER BY id";

type Version = (i32, String);
type Row = (String, String, String, i64, String, String);

fn config() -> Config {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    Config::parse("", Some(dsn), None).unwrap()
}

async fn run(db: &Database, statements: &[&'static str]) {
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in statements {
        sqlx::raw_sql(*statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
}

async fn versions(db: &Database) -> Vec<Version> {
    let mut tx = db.begin_catalog().await.unwrap();
    let rows = sqlx::query_as::<_, Version>(VERSIONS)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    rows
}

async fn datasets(db: &Database) -> Vec<Row> {
    let mut tx = db.begin_catalog().await.unwrap();
    let rows = sqlx::query_as::<_, Row>(DATASETS)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    rows
}

fn numbers(rows: &[Version]) -> Vec<i32> {
    rows.iter().map(|(version, _)| *version).collect()
}

// One sequential test: the steps rebuild the shared catalog schema, so parallel
// tests in this binary would race on it.
#[tokio::test]
async fn migration_versions_are_recorded_once_and_service_cannot_ddl() {
    let config = config();
    let db = Database::open(&config).await.unwrap();

    // Fresh schema, two concurrent runs: the advisory lock serializes them.
    run(&db, &["DROP SCHEMA IF EXISTS atlas_catalog CASCADE"]).await;
    let (first, second) = tokio::join!(migrate(&config), migrate(&config));
    first.unwrap();
    second.unwrap();
    let recorded = versions(&db).await;
    assert_eq!(numbers(&recorded), [1, 2, 3]);
    // Repeating changes nothing, not even applied_at.
    migrate(&config).await.unwrap();
    assert_eq!(versions(&db).await, recorded);
    assert!(datasets(&db).await.is_empty());

    // An installation that predates the version table: datasets with rows, no migrations.
    run(
        &db,
        &[
            "DROP SCHEMA atlas_catalog CASCADE",
            include_str!("../migrations/0001_catalog.sql"),
            r#"INSERT INTO atlas_catalog.datasets(id, config, metadata, revision, created_at, updated_at) VALUES
                ('legacy_a', '{"id":"legacy_a","title":"A"}', '{"srid":4326}', 3, '2020-01-01T00:00:00Z', '2021-02-03T04:05:06Z'),
                ('legacy_b', '{"id":"legacy_b"}', '{}', 1, '2019-06-01T00:00:00Z', '2019-06-01T00:00:00Z')"#,
        ],
    )
    .await;
    let legacy = datasets(&db).await;
    assert_eq!(legacy.len(), 2);
    migrate(&config).await.unwrap();
    assert_eq!(datasets(&db).await, legacy);
    let recorded = versions(&db).await;
    assert_eq!(numbers(&recorded), [1, 2, 3]);
    migrate(&config).await.unwrap();
    assert_eq!(datasets(&db).await, legacy);
    assert_eq!(versions(&db).await, recorded);
    run(
        &db,
        &["DELETE FROM atlas_catalog.datasets WHERE id IN ('legacy_a', 'legacy_b')"],
    )
    .await;

    // The service role reads the catalog and its version but cannot run DDL.
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DO $$ BEGIN IF NOT EXISTS(SELECT FROM pg_roles WHERE rolname='atlas_service_test') THEN CREATE ROLE atlas_service_test NOLOGIN; END IF; END $$",
        "GRANT USAGE ON SCHEMA atlas_catalog TO atlas_service_test",
        "GRANT SELECT, INSERT, UPDATE, DELETE ON atlas_catalog.datasets TO atlas_service_test",
        "GRANT SELECT ON atlas_catalog.migrations TO atlas_service_test",
        "SET LOCAL ROLE atlas_service_test",
        "SELECT * FROM atlas_catalog.datasets LIMIT 0",
        "SELECT max(version) FROM atlas_catalog.migrations",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    assert!(
        sqlx::query("CREATE TABLE atlas_catalog.denied(id integer)")
            .execute(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.close().await;
    upgrade_resolves_the_id_type_of_legacy_publications().await;
}

// Called from the sequential test above, which rebuilds the shared catalog schema.
async fn upgrade_resolves_the_id_type_of_legacy_publications() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let mut config = Config::parse("", Some(dsn), None).unwrap();
    config.allowed_schemas = vec!["migration_legacy".into()];
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS migration_legacy CASCADE",
        "CREATE SCHEMA migration_legacy",
        "CREATE TABLE migration_legacy.source(id integer PRIMARY KEY, name text, geom geometry(Point,4326))",
        "INSERT INTO migration_legacy.source VALUES (5, 'five', ST_SetSRID(ST_Point(10, 10), 4326))",
        "DELETE FROM atlas_catalog.datasets WHERE id IN ('legacy_test','legacy_missing')",
        // Pretend the id_type backfill has not run yet so the next migrate applies it.
        "DELETE FROM atlas_catalog.migrations WHERE version = 2",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let mut dataset: Dataset = serde_json::from_str(r#"{"id":"legacy_test","title":"Legacy","source":{"schema":"migration_legacy","table":"source"},"properties":["name"]}"#).unwrap();
    let fresh = inspect(&db, &config, &dataset).await.unwrap();
    // Metadata as stored before id_type existed.
    let mut legacy = serde_json::to_value(&fresh).unwrap();
    legacy.as_object_mut().unwrap().remove("id_type");
    let mut tx = db.begin_catalog().await.unwrap();
    for (id, table) in [("legacy_test", "source"), ("legacy_missing", "gone")] {
        dataset.id = id.into();
        dataset.source.table = table.into();
        sqlx::query("INSERT INTO atlas_catalog.datasets(id, config, metadata) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(Json(&dataset))
            .bind(Json(&legacy))
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();

    migrate(&config).await.unwrap();
    migrate(&config).await.unwrap();
    let catalog = Catalog::new(db.clone(), config.clone()).unwrap();
    assert_eq!(
        catalog.get("legacy_test").await.unwrap().metadata.id_type,
        "int4"
    );
    assert!(matches!(
        catalog.get("legacy_missing").await,
        Err(Error::SchemaChanged)
    ));

    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DELETE FROM atlas_catalog.datasets WHERE id IN ('legacy_test','legacy_missing')",
        "DROP SCHEMA migration_legacy CASCADE",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    db.close().await;
}
