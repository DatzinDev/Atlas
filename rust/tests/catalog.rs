use atlas::{
    catalog::Catalog,
    config::Config,
    domain::{Dataset, Error},
    migrations::migrate,
    postgis::connection::Database,
};

#[tokio::test]
async fn persistent_publications_compare_and_swap_and_source_survives() {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["catalog_test".into()];
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS catalog_test CASCADE",
        "CREATE SCHEMA catalog_test",
        "CREATE TABLE catalog_test.source(id bigint PRIMARY KEY, geom geometry(Point,4326))",
        "INSERT INTO catalog_test.source VALUES (1,ST_SetSRID(ST_Point(0,0),4326))",
        "DELETE FROM atlas_catalog.datasets WHERE id IN ('persist_test','invalid_test')",
        "DO $$ BEGIN IF NOT EXISTS(SELECT FROM pg_roles WHERE rolname='atlas_service_block') THEN CREATE ROLE atlas_service_block NOLOGIN; END IF; END $$",
        "GRANT USAGE ON SCHEMA atlas_catalog,catalog_test TO atlas_service_block",
        "GRANT SELECT ON catalog_test.source TO atlas_service_block",
        "GRANT SELECT,INSERT,UPDATE,DELETE ON atlas_catalog.datasets TO atlas_service_block",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    db.close().await;
    let owner = config.clone();
    let dsn = config.database_url.as_ref().unwrap();
    config.database_url = Some(format!(
        "{dsn}{}options=-crole%3Datlas_service_block",
        if dsn.contains('?') { "&" } else { "?" }
    ));
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_read().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT current_user::text")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        "atlas_service_block"
    );
    tx.rollback().await.unwrap();
    let mut dataset: Dataset = serde_json::from_str(r#"{"id":"persist_test","title":"Persistent","source":{"schema":"catalog_test","table":"source"},"properties":["id"]}"#).unwrap();
    let catalog = Catalog::new(db.clone(), config.clone()).unwrap();
    catalog.ready().await.unwrap();
    assert_eq!(catalog.create(&dataset).await.unwrap().revision, 1);
    assert!(matches!(
        catalog.create(&dataset).await,
        Err(Error::PublicationConflict)
    ));
    db.close().await;
    let db = Database::open(&config).await.unwrap();
    let catalog = Catalog::new(db.clone(), config.clone()).unwrap();
    assert_eq!(
        catalog.get(&dataset.id).await.unwrap().config.title,
        "Persistent"
    );
    dataset.title = "Updated".into();
    let (first, second) = tokio::join!(catalog.update(&dataset, 1), catalog.update(&dataset, 1));
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert!(
        matches!(first, Err(Error::RevisionMismatch))
            || matches!(second, Err(Error::RevisionMismatch))
    );
    assert!(matches!(
        catalog.update(&dataset, 1).await,
        Err(Error::RevisionMismatch)
    ));
    assert_eq!(
        catalog.delete(&dataset.id, 1).await,
        Err(Error::RevisionMismatch)
    );
    assert_eq!(
        catalog.get(&dataset.id).await.unwrap().config.title,
        "Updated"
    );
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("UPDATE atlas_catalog.datasets SET metadata=jsonb_set(metadata,'{srid}','0') WHERE id='persist_test'").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        catalog.get(&dataset.id).await,
        Err(Error::SchemaChanged)
    ));
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("UPDATE atlas_catalog.datasets SET metadata=jsonb_set(metadata,'{srid}','4326') WHERE id='persist_test'").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    catalog.delete(&dataset.id, 2).await.unwrap();
    assert!(matches!(
        catalog.get(&dataset.id).await,
        Err(Error::DatasetNotFound)
    ));
    dataset.id = "invalid_test".into();
    dataset.properties = vec!["missing".into()];
    assert!(catalog.create(&dataset).await.is_err());
    assert!(matches!(
        catalog.get(&dataset.id).await,
        Err(Error::DatasetNotFound)
    ));
    let mut tx = db.begin_read().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM catalog_test.source")
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        1
    );
    tx.rollback().await.unwrap();
    let cleanup = Database::open(&owner).await.unwrap();
    let mut tx = cleanup.begin_catalog().await.unwrap();
    sqlx::query("DROP SCHEMA catalog_test CASCADE")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    cleanup.close().await;
    db.close().await;
    assert_eq!(catalog.ready().await, Err(Error::DatabaseUnavailable));
}
