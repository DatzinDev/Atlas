use atlas::{
    api::admin::router,
    auth::{http::authenticate, jwt::Verifier},
    config::Config,
    migrations::migrate,
    postgis::connection::Database,
    services::publications::Publications,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, get_current_timestamp};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
};

const BODY: &str = r#"{"id":"api_roads","title":"Roads","source":{"schema":"admin_test","table":"roads"},"properties":["name"]}"#;
const LIST: &str = "/api/v1/admin/datasets";
const ITEM: &str = "/api/v1/admin/datasets/api_roads";

fn bearer(roles: &[&str]) -> String {
    let claims = serde_json::json!({"sub": "alice", "iss": "atlas-test", "aud": "atlas",
        "exp": get_current_timestamp() + 3600, "roles": roles});
    let key = EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwt-test-private.pem")).unwrap();
    let token = encode(&Header::new(Algorithm::RS256), &claims, &key).unwrap();
    format!("Authorization: Bearer {token}\r\n")
}

async fn expect(
    address: SocketAddr,
    request: (&str, &str, &str, &str),
    status: &str,
    needle: &str,
) {
    let (method, target, headers, body) = request;
    let raw = format!(
        "{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
        body.len()
    );
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    })
    .await
    .unwrap();
    let context = format!("{method} {target}: {response}");
    assert!(
        response.starts_with(&format!("HTTP/1.1 {status}")),
        "{context}"
    );
    assert!(response.to_lowercase().contains(needle), "{context}");
    if status.starts_with('4') {
        assert!(response.contains("request_id") && !response.contains("SELECT"));
    }
}

async fn server() -> (SocketAddr, Publications) {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["admin_test".into()];
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS admin_test CASCADE",
        "CREATE SCHEMA admin_test",
        "CREATE TABLE admin_test.roads(id integer PRIMARY KEY, name text, geom geometry(LineString,4326))",
        "DELETE FROM atlas_catalog.datasets WHERE config->'source'->>'schema' <> 'admin_test' OR id LIKE 'api\\_%'",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let verifier = Verifier::new(
        include_bytes!("fixtures/jwt-test-public.pem"),
        "atlas-test",
        "atlas",
    )
    .unwrap();
    let service = Publications::new(db, config).unwrap();
    let app = router(service.clone()).layer(axum::middleware::from_fn_with_state(
        Arc::new(Some(verifier)),
        authenticate,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, service)
}

#[tokio::test]
async fn admin_routes_over_real_http() {
    let (a, service) = server().await;
    service.create(BODY.as_bytes()).await.unwrap();
    let (admin, viewer) = (bearer(&["atlas-admin"]), bearer(&["viewer"]));
    let paged = |query: &str| format!("{LIST}?{query}");
    expect(a, ("GET", ITEM, "", ""), "403", "permission_denied").await;
    expect(a, ("GET", ITEM, &viewer, ""), "403", "permission_denied").await;
    expect(a, ("GET", ITEM, &admin, ""), "200", "etag: \"1\"").await;
    expect(
        a,
        ("GET", "/api/v1/admin/datasets/Bad", &admin, ""),
        "400",
        "invalid_request",
    )
    .await;
    expect(
        a,
        ("GET", "/api/v1/admin/datasets/api_no", &admin, ""),
        "404",
        "dataset_not_found",
    )
    .await;
    expect(
        a,
        ("GET", &paged("limit=1"), &admin, ""),
        "200",
        "\"hasmore\":false",
    )
    .await;
    expect(
        a,
        ("GET", &paged("offset=1"), &admin, ""),
        "200",
        "\"publications\":[]",
    )
    .await;
    for query in ["limit=0", "limit=x", "limit=1&limit=2", "sort=id", "limit"] {
        expect(
            a,
            ("GET", &paged(query), &admin, ""),
            "400",
            "invalid_request",
        )
        .await;
    }
}
