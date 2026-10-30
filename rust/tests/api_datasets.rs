use atlas::{
    api::datasets::router,
    auth::{http::authenticate, jwt::Verifier},
    config::Config,
    migrations::migrate,
    postgis::connection::Database,
    services::publications::Publications,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, get_current_timestamp};
const LIST: &str = "/api/v1/datasets";

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

async fn get(address: SocketAddr, target: &str, headers: &str) -> (String, serde_json::Value) {
    let raw =
        format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{headers}\r\n");
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    })
    .await
    .unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head[9..12].to_string();
    (status, serde_json::from_str(body).unwrap_or_default())
}

async fn page(a: SocketAddr, query: &str, headers: &str) -> (String, bool) {
    let (status, body) = get(a, &format!("{LIST}?{query}"), headers).await;
    assert_eq!(status, "200", "{body}");
    let text = body.to_string();
    for hidden in "source filter roles discovery_test access revision".split(' ') {
        assert!(!text.contains(hidden), "{hidden}: {text}");
    }
    let ids: Vec<_> = body["datasets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    (ids.join(","), body["hasMore"] == true)
}

async fn server() -> (SocketAddr, Publications) {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["discovery_test".into()];
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS discovery_test CASCADE",
        "CREATE SCHEMA discovery_test",
        "CREATE TABLE discovery_test.places(id integer PRIMARY KEY, name text, geom geometry(Point,4326))",
        "DELETE FROM atlas_catalog.datasets WHERE config->'source'->>'schema' <> 'discovery_test' OR id LIKE 'disc\\_%'",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db.clone(), config).unwrap();
    for (id, access, enabled) in [
        ("disc_a_public", r#"{"public":true}"#, true),
        (
            "disc_b_view",
            r#"{"view_roles":["viewer"],"query_roles":["querier"]}"#,
            true,
        ),
        (
            "disc_c_query",
            r#"{"view_roles":["viewer"],"query_roles":["viewer"]}"#,
            true,
        ),
        ("disc_d_disabled", r#"{"public":true}"#, false),
    ] {
        let body = format!(
            r#"{{"id":"{id}","title":"T","source":{{"schema":"discovery_test","table":"places"}},"properties":["name"],"filter":{{"op":"is_not_null","field":"name"}},"access":{access},"enabled":{enabled}}}"#
        );
        service.create(body.as_bytes()).await.unwrap();
    }
    let broken = r#"INSERT INTO atlas_catalog.datasets(id, config, metadata) SELECT 'disc_e_broken', jsonb_set(jsonb_set(config, '{id}', '"disc_e_broken"'), '{source,schema}', '"elsewhere"'), metadata FROM atlas_catalog.datasets WHERE id = 'disc_a_public'"#;
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query(broken).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let verifier = Verifier::new(
        include_bytes!("fixtures/jwt-test-public.pem"),
        "atlas-test",
        "atlas",
    )
    .unwrap();
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
async fn discovery_lists_only_authorized_datasets_over_real_http() {
    let (a, service) = server().await;
    let (viewer, admin) = (bearer(&["viewer"]), bearer(&["atlas-admin"]));
    let all = "disc_a_public,disc_b_view,disc_c_query";
    assert_eq!(page(a, "", "").await, ("disc_a_public".into(), false));
    assert_eq!(
        page(a, "limit=1", "").await,
        ("disc_a_public".into(), false)
    );
    assert_eq!(page(a, "", &admin).await, ("disc_a_public".into(), false));
    assert_eq!(page(a, "", &viewer).await, (all.into(), false));
    assert_eq!(
        page(a, "limit=1&offset=1", &viewer).await,
        ("disc_b_view".into(), true)
    );
    assert_eq!(
        page(a, "limit=1&offset=2", &viewer).await,
        ("disc_c_query".into(), false)
    );
    let (_, detail) = get(a, &format!("{LIST}/disc_a_public"), "").await;
    assert_eq!(detail["attributes"][0]["name"], "name");
    assert!(detail.get("source").is_none() && detail.get("revision").is_none());

    for (target, headers, expected) in [
        ("/disc_b_view", "", "404"),
        ("/disc_b_view", viewer.as_str(), "403"),
        ("/disc_c_query", viewer.as_str(), "200"),
        ("/disc_d_disabled", "", "404"),
        ("/disc_d_disabled", admin.as_str(), "404"),
        ("/disc_none", viewer.as_str(), "404"),
        ("/disc_e_broken", "", "404"),
        ("/Bad", "", "400"),
        ("?limit=0", "", "400"),
        ("?limit=1001", "", "400"),
        ("?offset=100001", "", "400"),
        ("?sort=id", "", "400"),
        ("", "Authorization: Bearer forged\r\n", "401"),
    ] {
        let (status, body) = get(a, &format!("{LIST}{target}"), headers).await;
        assert_eq!(status, expected, "{target}: {body}");
        assert!(!body.to_string().contains("discovery_test"));
    }

    let public = br#"{"access":{"public":true}}"#;
    service.update("disc_b_view", public, 1).await.unwrap();
    assert_eq!(page(a, "", "").await.0, "disc_a_public,disc_b_view");
}
