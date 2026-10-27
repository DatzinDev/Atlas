use atlas::{config::Config, migrations::migrate, postgis::connection::Database};

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
