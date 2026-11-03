use atlas::{
    api::features::router,
    auth::{http::authenticate, jwt::Verifier},
    config::Config,
    migrations::migrate,
    postgis::connection::Database,
    services::publications::Publications,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, get_current_timestamp};
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
};

fn bearer(roles: &[&str]) -> String {
    let claims = serde_json::json!({"sub": "alice", "iss": "atlas-test", "aud": "atlas",
        "exp": get_current_timestamp() + 3600, "roles": roles});
    let key = EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwt-test-private.pem")).unwrap();
    let token = encode(&Header::new(Algorithm::RS256), &claims, &key).unwrap();
    format!("Authorization: Bearer {token}\r\n")
}

async fn call(
    a: SocketAddr,
    method: &str,
    target: &str,
    headers: &str,
    body: &str,
) -> (String, String, Value) {
    let raw = format!(
        "{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
        body.len()
    );
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(a).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    })
    .await
    .unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (
        head[9..12].to_owned(),
        head.to_lowercase(),
        serde_json::from_str(body).unwrap_or_default(),
    )
}

fn ids(body: &Value) -> Vec<i64> {
    body["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_i64().unwrap())
        .collect()
}

async fn server() -> (SocketAddr, Database) {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["items_test".into()];
    config.limits.query_body_bytes = 512;
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS items_test CASCADE",
        "CREATE SCHEMA items_test",
        "CREATE TABLE items_test.places(id integer PRIMARY KEY, name text, rank int4, secret text, geom geometry(Point,4326))",
        "INSERT INTO items_test.places SELECT n, 'p' || n, n % 3, 'x', ST_SetSRID(ST_Point(n, n), 4326) FROM generate_series(1, 9) AS n",
        "CREATE TABLE items_test.doomed AS TABLE items_test.places",
        "ALTER TABLE items_test.doomed ADD PRIMARY KEY (id)",
        "DELETE FROM atlas_catalog.datasets WHERE config->'source'->>'schema' <> 'items_test' OR id LIKE 'items\\_%'",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db.clone(), config).unwrap();
    let both = r#"{"view_roles":["viewer"],"query_roles":["viewer"]}"#;
    for (id, table, access) in [
        ("items_public", "places", r#"{"public":true}"#),
        ("items_doomed", "doomed", r#"{"public":true}"#),
        ("items_view", "places", r#"{"view_roles":["viewer"]}"#),
        ("items_query", "places", both),
    ] {
        let body = format!(
            r#"{{"id":"{id}","title":"T","source":{{"schema":"items_test","table":"{table}"}},"properties":["name","rank"],"filter":{{"op":"gte","field":"rank","value":1}},"access":{access}}}"#
        );
        service.create(body.as_bytes()).await.unwrap();
    }
    let verifier = Verifier::new(
        include_bytes!("fixtures/jwt-test-public.pem"),
        "atlas-test",
        "atlas",
    )
    .unwrap();
    let app = router(service).layer(axum::middleware::from_fn_with_state(
        Arc::new(Some(verifier)),
        authenticate,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, db)
}

#[tokio::test]
async fn items_and_query_over_real_http() {
    let (a, db) = server().await;
    let viewer = bearer(&["viewer"]);
    let items = "/api/v1/datasets/items_public/items";
    let query = "/api/v1/datasets/items_public/query";

    let (status, head, page) = call(a, "GET", &format!("{items}?limit=2"), "", "").await;
    assert_eq!(status, "200", "{head}");
    assert!(head.contains("application/geo+json") && head.contains("no-store"));
    assert_eq!(page["type"], "FeatureCollection");
    assert_eq!(ids(&page), [1, 2]);
    assert!(page["numberReturned"] == 2 && page["hasMore"] == true);
    assert!(page.to_string().contains("\"name\":\"p1\"") && !page.to_string().contains("secret"));

    let filter = "%7B%22op%22%3A%22lte%22%2C%22field%22%3A%22rank%22%2C%22value%22%3A1%7D";
    let get = format!(
        "{items}?filter={filter}&bbox=0,0,8.5,8.5&properties=name&sort=name:desc&limit=3&offset=1"
    );
    let post = r#"{"filter":{"op":"lte","field":"rank","value":1},"bbox":[0,0,8.5,8.5],"properties":["name"],"sort":[{"field":"name","direction":"desc"}],"limit":3,"offset":1}"#;
    let (_, _, by_get) = call(a, "GET", &get, "", "").await;
    let (status, _, by_post) = call(a, "POST", query, "", post).await;
    assert_eq!(status, "200");
    assert_eq!(by_get, by_post);
    assert!(ids(&by_get) == [4, 1] && by_get["hasMore"] == false);
    let first = &by_get["features"][0]["properties"];
    assert_eq!(first, &serde_json::json!({"name": "p4"}));
    let zero = r#"{"filter":{"op":"eq","field":"rank","value":0}}"#;
    let (_, _, empty) = call(a, "POST", query, "", zero).await;
    assert!(ids(&empty).is_empty() && empty["numberReturned"] == 0);

    let view = "/api/v1/datasets/items_view/items";
    let missing = "/api/v1/datasets/items_none/items";
    let missing_query = "/api/v1/datasets/items_none/query";
    let bogus = "%7B%22op%22%3A%22bogus%22%7D";
    let unknown = "%7B%22op%22%3A%22is_null%22%2C%22field%22%3A%22secret%22%7D";
    let granted = "/api/v1/datasets/items_query/items";
    for (method, target, headers, body, expected) in [
        ("GET", view, "", "", "404"),
        ("GET", view, viewer.as_str(), "", "403"),
        ("GET", granted, viewer.as_str(), "", "200"),
        ("GET", "/api/v1/datasets/items_none/items", "", "", "404"),
        ("GET", &format!("{missing}?filter=%7B"), "", "", "400"),
        ("GET", &format!("{missing}?filter={bogus}"), "", "", "400"),
        ("GET", &format!("{missing}?filter={unknown}"), "", "", "404"),
        (
            "POST",
            missing_query,
            "",
            r#"{"filter":{"op":"bogus"}}"#,
            "400",
        ),
        ("POST", &format!("{query}?unknown=1"), "", "{}", "400"),
        ("GET", &format!("{items}?limit=1&limit=2"), "", "", "400"),
        ("GET", &format!("{items}?unknown=1"), "", "", "400"),
        ("GET", &format!("{items}?bbox=0,0,1"), "", "", "400"),
        ("GET", &format!("{items}?bbox=2,0,1,1"), "", "", "400"),
        ("GET", &format!("{items}?sort=name:up"), "", "", "400"),
        ("GET", &format!("{items}?properties=secret"), "", "", "400"),
        ("GET", &format!("{items}?limit=10001"), "", "", "400"),
        ("GET", &format!("{items}?filter=%7B"), "", "", "400"),
        (
            "GET",
            &format!("{items}?filter=%7B%22op%22%3A%22is_null%22%2C%22field%22%3A%22secret%22%7D"),
            "",
            "",
            "400",
        ),
        ("POST", query, "", r#"{"limit":1,"extra":1}"#, "400"),
        ("POST", query, "", r#"{"limit":1,"limit":2}"#, "400"),
        (
            "POST",
            query,
            "",
            r#"{"filter":{"op":"eq","field":"rank","value":"1"}}"#,
            "400",
        ),
        (
            "POST",
            query,
            "",
            &format!(r#"{{"properties":["{}"]}}"#, "n".repeat(600)),
            "413",
        ),
    ] {
        let (status, _, body) = call(a, method, target, headers, body).await;
        assert_eq!(status, expected, "{method} {target}: {body}");
        let failed = body.get("features").is_none() && body["request_id"].is_string();
        assert!(expected == "200" || failed, "{body}");
        let filtered = expected == "400" && target.contains("items_none");
        assert!(!filtered || body["code"] == "invalid_filter", "{target}");
    }

    let (status, _, body) = call(a, "POST", &format!("{missing_query}?unknown=1"), "", "{}").await;
    assert_eq!(
        (status.as_str(), &body["code"]),
        ("400", &"invalid_request".into())
    );
    assert!(body["request_id"].is_string());

    let doomed = "/api/v1/datasets/items_doomed/items";
    assert_eq!(call(a, "GET", doomed, "", "").await.0, "200");
    for statement in [
        "ALTER TABLE items_test.doomed DROP COLUMN name",
        "DROP TABLE items_test.doomed",
    ] {
        let mut tx = db.begin_catalog().await.unwrap();
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        let (status, _, body) = call(a, "GET", doomed, "", "").await;
        assert_eq!(status, "409", "{body}");
        assert_eq!(body["code"], "schema_changed");
    }
}
