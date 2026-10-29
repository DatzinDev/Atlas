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
        "ALTER TABLE publication_test.roads ADD amount numeric",
        "DELETE FROM atlas_catalog.datasets WHERE config->'source'->>'schema' <> 'publication_test' OR id LIKE 'svc\\_%'",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    (db.clone(), Publications::new(db, config).unwrap())
}

#[tokio::test]
async fn publications_validate_before_persisting_and_patch_with_revision() {
    let (db, service) = setup().await;
    let created = service.create(ROADS.as_bytes()).await.unwrap();
    let numbers = ROADS
        .replace("svc_roads", "svc_numeric")
        .replace(r#"["name","lanes"]"#, r#"["name","lanes","amount"]"#);
    service.create(numbers.as_bytes()).await.unwrap();
    let numeric = service
        .update(
            "svc_numeric",
            br#"{"filter":{"op":"eq","field":"amount","value":12345678901234567890.123}}"#,
            1,
        )
        .await
        .unwrap();
    assert!(
        numeric
            .config
            .filter
            .as_ref()
            .unwrap()
            .get()
            .contains("12345678901234567890.123")
    );
    service
        .delete("svc_numeric", numeric.revision)
        .await
        .unwrap();
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

    let patched = service
        .update(
            "svc_roads",
            br#"{"title":"New","description":null,"access":{"public":true}}"#,
            1,
        )
        .await
        .unwrap();
    assert_eq!(patched.revision, 2);
    assert_eq!(patched.config.title, "New");
    assert_eq!(patched.config.properties, ["name", "lanes"]);
    assert!(patched.config.access.public && patched.config.access.view_roles == ["viewer"]);
    let replaced = service
        .update(
            "svc_roads",
            br#"{"filter":{"op":"is_null","field":"name"}}"#,
            2,
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(replaced.config.filter.unwrap().get()).unwrap(),
        serde_json::json!({"op": "is_null", "field": "name"})
    );
    for (body, revision, expected) in [
        (&br#"{"title":"Stale"}"#[..], 2, Error::RevisionMismatch),
        (br#"{"id":"svc_renamed"}"#, 3, Error::InvalidRequest),
        (br#"{"title":null}"#, 3, Error::InvalidRequest),
        (br#"{"unknown":1}"#, 3, Error::InvalidRequest),
        (br#"{"title":"A","title":"B"}"#, 3, Error::InvalidRequest),
        (br#"["title"]"#, 3, Error::InvalidRequest),
        (br#"{"filter":{"op":"eq","field":"lanes","value":{"$serde_json::private::Number":"2"}}}"#, 3, Error::InvalidFilter),
        (br#"{"filter":{"op":"in","field":"lanes","value":[{"$serde_json::private::Number":"2"}]}}"#, 3, Error::InvalidFilter),
        (
            br#"{"filter":{"op":"eq","field":"secret","value":1}}"#,
            3,
            Error::InvalidFilter,
        ),
    ] {
        assert_eq!(
            service.update("svc_roads", body, revision).await.err(),
            Some(expected)
        );
    }
    let cleared = service
        .update("svc_roads", br#"{"filter":null}"#, 3)
        .await
        .unwrap();
    assert!(cleared.config.filter.is_none() && cleared.revision == 4);

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
        service.delete("svc_roads", 3).await.err(),
        Some(Error::RevisionMismatch)
    );
    service.delete("svc_roads", 4).await.unwrap();
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
