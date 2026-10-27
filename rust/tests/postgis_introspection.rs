use atlas::{
    config::Config,
    domain::Dataset,
    postgis::{connection::Database, introspection::inspect},
};

#[tokio::test]
async fn real_relations_geometry_identifiers_and_properties() {
    let config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS introspection_test CASCADE",
        "CREATE SCHEMA introspection_test",
        "CREATE TABLE introspection_test.\"quoted\"\"table\"(\"weird\"\"id\" bigint PRIMARY KEY, name text, payload bytea, geom geometry(Point,4326))",
        "CREATE TABLE introspection_test.ambiguous(id integer PRIMARY KEY, geom geometry(Point,4326), other geometry(Point,3857))",
        "CREATE TABLE introspection_test.generic(id integer PRIMARY KEY, geom geometry)",
        "CREATE TABLE introspection_test.composite(id integer, second integer, geom geometry(Point,4326), PRIMARY KEY(id,second))",
        "CREATE VIEW introspection_test.good AS SELECT \"weird\"\"id\" AS id, geom FROM introspection_test.\"quoted\"\"table\"",
        "CREATE VIEW introspection_test.duplicate AS SELECT 1 AS id, ST_SetSRID(ST_Point(0,0),4326)::geometry(Point,4326) AS geom FROM generate_series(1,2)",
        "CREATE VIEW introspection_test.null_id AS SELECT NULL::integer AS id, ST_SetSRID(ST_Point(0,0),4326)::geometry(Point,4326) AS geom",
        "CREATE MATERIALIZED VIEW introspection_test.materialized AS SELECT * FROM introspection_test.good",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let mut config = config;
    config.allowed_schemas = vec!["introspection_test".into()];
    let mut dataset: Dataset = serde_json::from_str(r#"{"id":"test","title":"Test","source":{"schema":"introspection_test","table":"quoted\"table"},"properties":["weird\"id","name"]}"#).unwrap();
    let meta = inspect(&db, &config, &dataset).await.unwrap();
    assert_eq!(meta.srid, 4326);
    assert_eq!(meta.id_column, "weird\"id");
    assert_eq!(meta.attributes.len(), 2);
    dataset.properties = vec!["payload".into()];
    assert!(inspect(&db, &config, &dataset).await.is_err());
    dataset.properties = vec!["geom".into()];
    assert!(inspect(&db, &config, &dataset).await.is_err());
    dataset.source.table = "ambiguous".into();
    dataset.properties = vec!["id".into()];
    assert!(inspect(&db, &config, &dataset).await.is_err());
    dataset.source.geometry_column = Some("other".into());
    assert_eq!(inspect(&db, &config, &dataset).await.unwrap().srid, 3857);
    dataset.source.geometry_column = None;
    for table in [
        "generic",
        "composite",
        "good",
        "duplicate",
        "null_id",
        "not_exists",
        "x'; DROP SCHEMA public CASCADE--",
    ] {
        dataset.source.table = table.into();
        assert!(inspect(&db, &config, &dataset).await.is_err());
    }
    dataset.source.id_column = Some("id".into());
    for table in ["good", "materialized"] {
        dataset.source.table = table.into();
        assert_eq!(
            inspect(&db, &config, &dataset).await.unwrap().id_column,
            "id"
        );
    }
    for table in ["duplicate", "null_id"] {
        dataset.source.table = table.into();
        assert!(inspect(&db, &config, &dataset).await.is_err());
    }
    dataset.source.schema = "pg_catalog".into();
    assert!(inspect(&db, &config, &dataset).await.is_err());
    dataset.source.schema = "introspection_test".into();
    dataset.source.table = "generic".into();
    dataset.source.id_column = None;
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query(
        "INSERT INTO introspection_test.generic VALUES (1, ST_SetSRID(ST_Point(0,0),4326))",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(inspect(&db, &config, &dataset).await.unwrap().srid, 4326);
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("DROP SCHEMA introspection_test CASCADE")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    db.close().await;
}
