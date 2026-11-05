use atlas::{
    api::export::router,
    auth::{http::authenticate, jwt::Verifier},
    config::Config,
    migrations::migrate,
    postgis::connection::Database,
    services::publications::Publications,
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
};

async fn request(a: SocketAddr, path: &str, headers: &str, abandon: bool) -> (String, String) {
    let request = format!(
        "GET /api/v1/datasets/{path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{headers}\r\n"
    );
    tokio::task::spawn_blocking(move || {
        let mut s = TcpStream::connect(a).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        s.write_all(request.as_bytes()).unwrap();
        if abandon {
            let mut buf = [0; 200];
            let _ = s.read(&mut buf);
            return (String::new(), String::new());
        }
        let mut text = String::new();
        s.read_to_string(&mut text).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let mut decoded = String::new();
        if head.to_lowercase().contains("transfer-encoding: chunked") {
            let mut rest = body;
            loop {
                let (len, data) = rest.split_once("\r\n").unwrap();
                let n = usize::from_str_radix(len, 16).unwrap();
                if n == 0 {
                    break;
                }
                decoded.push_str(&data[..n]);
                rest = &data[n + 2..];
            }
        } else {
            decoded.push_str(body);
        }
        (head.to_lowercase(), decoded)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn downloads_over_http() {
    let mut c = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").unwrap()),
        None,
    )
    .unwrap();
    c.allowed_schemas = vec!["http_export".into()];
    c.limits.default_limit = 1;
    c.limits.pool_max = 2;
    c.limits.export_features = 10;
    migrate(&c).await.unwrap();
    let db = Database::open(&c).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS http_export CASCADE",
        "CREATE SCHEMA http_export",
        "DELETE FROM atlas_catalog.datasets WHERE id LIKE 'export_%'",
        "CREATE TABLE http_export.data(id bigint PRIMARY KEY, n numeric, geom geometry(Point,4326))",
        "INSERT INTO http_export.data SELECT n, 12345678901234567890.123, ST_SetSRID(ST_Point(n,n),4326) FROM generate_series(1,9) n",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db.clone(), c).unwrap();
    for (id, access) in [
        ("export_public", r#"{"public":true,"public_download":true}"#),
        (
            "export_private",
            r#"{"view_roles":["download"],"download_roles":["download"]}"#,
        ),
        ("export_denied", r#"{"public":true}"#),
    ] {
        let body = format!(
            r#"{{"id":"{id}","title":"X","source":{{"schema":"http_export","table":"data"}},"properties":["id","n"],"filter":{{"op":"gte","field":"id","value":2}},"access":{access}}}"#
        );
        service.create(body.as_bytes()).await.unwrap();
    }
    let verifier = Verifier::new(
        include_bytes!("fixtures/jwt-test-public.pem"),
        "test",
        "test",
    )
    .unwrap();
    let app = router(service).layer(axum::middleware::from_fn_with_state(
        Arc::new(Some(verifier)),
        authenticate,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let claims = serde_json::json!({"sub":"a","iss":"test","aud":"test","exp":jsonwebtoken::get_current_timestamp()+60,"roles":["download"]});
    let key =
        jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwt-test-private.pem"))
            .unwrap();
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        &key,
    )
    .unwrap();
    let auth = format!("Authorization: Bearer {token}\r\n");
    for (path, headers, status) in [
        ("export_public/export", "", "200"),
        ("export_private/export", "", "404"),
        ("export_private/export", auth.as_str(), "200"),
        ("export_denied/export", "", "403"),
        (
            "export_public/export",
            "Authorization: Bearer invalid\r\n",
            "401",
        ),
        ("export_public/export?limit=1", "", "400"),
        ("export_public/export?offset=0", "", "400"),
        ("export_public/export?sort=id:asc", "", "400"),
        ("export_public/export?format=csv", "", "400"),
        (
            "export_public/export?format=geojson&format=geojson",
            "",
            "400",
        ),
        ("export_public/export?bbox=0,0,8,8&bbox=0,0,8,8", "", "400"),
        ("export_public/export?properties=geom", "", "400"),
    ] {
        let (head, body) = request(a, path, headers, false).await;
        assert!(
            head.starts_with(&format!("http/1.1 {status}")),
            "{path}: {head} {body}"
        );
        assert!(head.contains("no-store"));
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        if status == "200" {
            assert!(
                head.contains("attachment; filename=\"export_")
                    && head.contains("application/geo+json")
            );
            assert_eq!(json["features"].as_array().unwrap().len(), 8);
            assert!(body.contains("12345678901234567890.123"));
        } else {
            assert!(json["request_id"].is_string());
        }
    }
    let filter = "%7B%22op%22%3A%22lte%22%2C%22field%22%3A%22id%22%2C%22value%22%3A4%7D";
    let (_, body) = request(
        a,
        &format!("export_public/export?format=geojson&bbox=0,0,3,3&properties=n&filter={filter}"),
        "",
        false,
    )
    .await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["features"].as_array().unwrap().len(), 2);
    for _ in 0..8 {
        request(a, "export_public/export", "", true).await;
    }
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("INSERT INTO http_export.data SELECT n, n, NULL FROM generate_series(10,20) n")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (head, body) = request(a, "export_public/export", "", false).await;
    assert!(head.starts_with("http/1.1 413") && body.contains("query_too_large"));
    server.abort();
    db.close().await;
}
