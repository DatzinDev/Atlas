use atlas::{
    config::Config,
    domain::Error,
    migrations::migrate,
    postgis::connection::Database,
    services::publications::{Publications, document},
};

const ROADS: &str = r#"{"id":"svc_roads","title":"Roads","source":{"schema":"publication_test","table":"roads"},"properties":["name","lanes"],"filter":{"op":"gte","field":"lanes","value":2},"access":{"view_roles":["viewer"]}}"#;

async fn setup() -> (Database, Publications) {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["publication_test".into()];
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS publication_test CASCADE",
        "CREATE SCHEMA publication_test",
        "CREATE TABLE publication_test.roads(id integer PRIMARY KEY, name text, lanes int2, geom geometry(LineString,4326))",
        "INSERT INTO publication_test.roads VALUES (1,'a',2,NULL)",
        "DELETE FROM atlas_catalog.datasets WHERE id LIKE 'svc\\_%'",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    (db.clone(), Publications::new(db, config).unwrap())
}

#[tokio::test]
async fn publications_validate_before_persisting() {
    let (db, service) = setup().await;
    let created = service.create(ROADS.as_bytes()).await.unwrap();
    assert_eq!(created.revision, 1);
    let doc = document(&created);
    assert_eq!(doc["metadata"]["srid"], 4326);
    assert_eq!(doc["metadata"]["attributes"][1]["type"], "int2");
    assert_eq!(doc["config"]["filter"]["op"], "gte");
    assert_eq!(
        service.create(ROADS.as_bytes()).await.err(),
        Some(Error::PublicationConflict)
    );

    let other = |from: &str, to: &str| ROADS.replace("svc_roads", "svc_other").replace(from, to);
    for (body, expected) in [
        (
            other("\"access\"", "\"extra\":1,\"access\""),
            Error::InvalidRequest,
        ),
        (
            other("\"title\":\"Roads\"", "\"title\":\"A\",\"title\":\"B\""),
            Error::InvalidRequest,
        ),
        (other("svc_other", "Svc_Other"), Error::InvalidRequest),
        (
            other("publication_test\"", "public\""),
            Error::InvalidRequest,
        ),
        (other("\"roads\"", "\"missing\""), Error::InvalidRequest),
        (
            other("\"lanes\",\"value\":2", "\"id\",\"value\":2"),
            Error::InvalidFilter,
        ),
        (
            other("\"value\":2", "\"value\":\"2\""),
            Error::InvalidFilter,
        ),
        (
            other(
                r#"{"op":"gte","field":"lanes","value":2}"#,
                r#"{"op":"within","geometry":{"type":"Polygon","coordinates":[[[0,0],[2,2],[2,0],[0,2],[0,0]]]}}"#,
            ),
            Error::InvalidFilter,
        ),
    ] {
        assert_eq!(
            service.create(body.as_bytes()).await.err(),
            Some(expected),
            "{body}"
        );
    }
    assert_eq!(
        service.get("svc_other").await.err(),
        Some(Error::DatasetNotFound)
    );

    service
        .create(ROADS.replace("svc_roads", "svc_second").as_bytes())
        .await
        .unwrap();
    let (page, more) = service.list(Some(1000), 0).await.unwrap();
    let ids: Vec<_> = page
        .iter()
        .map(|p| p.config.id.as_str())
        .filter(|id| id.starts_with("svc_"))
        .collect();
    assert_eq!((ids, more), (vec!["svc_roads", "svc_second"], false));
    assert!(service.list(Some(1), 0).await.unwrap().1);
    for (limit, offset) in [(Some(0), 0), (Some(1001), 0), (None, 100_001)] {
        assert_eq!(
            service.list(limit, offset).await.err(),
            Some(Error::InvalidRequest)
        );
    }

    assert_eq!(
        service.delete("svc_roads", 2).await.err(),
        Some(Error::RevisionMismatch)
    );
    service.delete("svc_roads", 1).await.unwrap();
    assert_eq!(
        service.get("svc_roads").await.err(),
        Some(Error::DatasetNotFound)
    );
    let mut tx = db.begin_read().await.unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM publication_test.roads")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}
