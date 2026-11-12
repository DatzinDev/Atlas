use atlas::{
    catalog::Catalog,
    config::Config,
    domain::{Dataset, Error},
    migrations::migrate,
    postgis::{connection::Database, introspection::inspect},
};
use sqlx::types::Json;

#[tokio::test]
async fn explicit_migration_is_repeatable_and_service_cannot_ddl() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let config = Config::parse("", Some(dsn), None).unwrap();
    migrate(&config).await.unwrap();
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DO $$ BEGIN IF NOT EXISTS(SELECT FROM pg_roles WHERE rolname='atlas_service_test') THEN CREATE ROLE atlas_service_test NOLOGIN; END IF; END $$",
        "GRANT USAGE ON SCHEMA atlas_catalog TO atlas_service_test",
        "GRANT SELECT, INSERT, UPDATE, DELETE ON atlas_catalog.datasets TO atlas_service_test",
        "SET LOCAL ROLE atlas_service_test",
        "SELECT * FROM atlas_catalog.datasets LIMIT 0",
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
}

#[tokio::test]
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
