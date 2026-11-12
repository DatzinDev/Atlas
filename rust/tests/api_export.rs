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
    c.limits.data_operations = 1;
    c.limits.export_operations = 1;
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
    let app = router(service.clone()).layer(axum::middleware::from_fn_with_state(
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
    let mut held = service
        .export(
            "export_public",
            &atlas::domain::Principal::default(),
            &atlas::domain::Query::default(),
            None,
        )
        .await
        .unwrap();
    for (path, status) in [
        ("export_public/export", "503"),
        ("export_missing/export", "404"),
        ("export_private/export", "404"),
        ("export_denied/export", "403"),
        (
            "export_missing/export?filter=%7B%22op%22%3A%22bogus%22%7D",
            "400",
        ),
        (
            "export_private/export?filter=%7B%22op%22%3A%22bogus%22%7D",
            "400",
        ),
    ] {
        let (head, body) = request(a, path, "", false).await;
        assert!(
            head.starts_with(&format!("http/1.1 {status}")),
            "{path}: {head} {body}"
        );
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(json["request_id"].is_string());
        if status == "400" {
            assert_eq!(json["code"], "invalid_filter");
        }
    }
    while held.next().await.unwrap().is_some() {}
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

#[tokio::test]
async fn hides_invalid_stored_private_publication() {
    let mut c = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").unwrap()),
        None,
    )
    .unwrap();
    c.allowed_schemas = vec!["http_export_stale".into()];
    migrate(&c).await.unwrap();
    let db = Database::open(&c).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS http_export_stale CASCADE",
        "CREATE SCHEMA http_export_stale",
        "DELETE FROM atlas_catalog.datasets WHERE id = 'stale_private'",
        "CREATE TABLE http_export_stale.data(id bigint PRIMARY KEY, geom geometry(Point,4326))",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db.clone(), c).unwrap();
    service
        .create(br#"{"id":"stale_private","title":"X","source":{"schema":"http_export_stale","table":"data"},"properties":["id"],"access":{"view_roles":["v"],"download_roles":["d"]}}"#)
        .await
        .unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("UPDATE atlas_catalog.datasets SET metadata=jsonb_set(metadata,'{srid}','0') WHERE id='stale_private'")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let verifier = Verifier::new(
        include_bytes!("fixtures/jwt-test-public.pem"),
        "test",
        "test",
    )
    .unwrap();
    let app = router(service.clone())
        .merge(atlas::api::admin::router(service.clone(), 1 << 20))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(Some(verifier)),
            authenticate,
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (head, body) = request(a, "stale_private/export?format=geojson", "", false).await;
    assert!(head.starts_with("http/1.1 404"), "{head} {body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["code"], "dataset_not_found");
    let claims = serde_json::json!({"sub":"a","iss":"test","aud":"test","exp":jsonwebtoken::get_current_timestamp()+60,"roles":["atlas-admin"]});
    let key =
        jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwt-test-private.pem"))
            .unwrap();
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        &key,
    )
    .unwrap();
    let raw = format!(
        "GET /api/v1/admin/datasets/stale_private HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n\r\n"
    );
    let text = tokio::task::spawn_blocking(move || {
        let mut s = TcpStream::connect(a).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut text = String::new();
        s.read_to_string(&mut text).unwrap();
        text
    })
    .await
    .unwrap();
    assert!(text.starts_with("HTTP/1.1 409"), "{text}");
    server.abort();
    db.close().await;
}

/// Head, raw chunked body and its decoded complete chunks, read until the server drops the connection.
async fn cut_request(a: SocketAddr, path: &str) -> (String, String, String) {
    let request = format!(
        "GET /api/v1/datasets/{path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    );
    tokio::task::spawn_blocking(move || {
        let mut s = TcpStream::connect(a).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        let mut buf = [0; 8192];
        // An aborted stream may end in a reset instead of EOF; both end the body.
        while let Ok(n @ 1..) = s.read(&mut buf) {
            raw.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let mut decoded = String::new();
        let mut rest = body;
        while let Some((len, data)) = rest.split_once("\r\n") {
            match usize::from_str_radix(len, 16) {
                Ok(n) if n > 0 && data.len() >= n + 2 => {
                    decoded.push_str(&data[..n]);
                    rest = &data[n + 2..];
                }
                _ => break,
            }
        }
        (head.to_lowercase(), body.to_owned(), decoded)
    })
    .await
    .unwrap()
}

/// Retries while the export slot is still busy (503): its release after a cut is asynchronous.
async fn admitted(a: SocketAddr, path: &str) -> (String, String, String) {
    for _ in 0..500 {
        let response = cut_request(a, path).await;
        if !response.0.starts_with("http/1.1 503") {
            return response;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{path}: export slot never released");
}

async fn serve(service: Publications) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let app = router(service).layer(axum::middleware::from_fn_with_state(
        Arc::new(None::<Verifier>),
        authenticate,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = listener.local_addr().unwrap();
    (
        a,
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
    )
}

#[tokio::test]
async fn cuts_oversized_downloads_mid_stream() {
    let mut c = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").unwrap()),
        None,
    )
    .unwrap();
    c.allowed_schemas = vec!["http_export_batch".into()];
    c.limits.data_operations = 1;
    c.limits.export_operations = 1;
    c.limits.pool_max = 2;
    c.limits.export_bytes = 65536;
    migrate(&c).await.unwrap();
    let db = Database::open(&c).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS http_export_batch CASCADE",
        "CREATE SCHEMA http_export_batch",
        "DELETE FROM atlas_catalog.datasets WHERE id IN ('batch_big','batch_small')",
        "CREATE TABLE http_export_batch.big(id bigint PRIMARY KEY, label text, geom geometry(Point,4326))",
        "INSERT INTO http_export_batch.big SELECT 1, repeat('b', 1000000), ST_SetSRID(ST_Point(1,1),4326)",
        "CREATE TABLE http_export_batch.small(id bigint PRIMARY KEY, label text, geom geometry(Point,4326))",
        "INSERT INTO http_export_batch.small SELECT n, repeat('s', 150), ST_SetSRID(ST_Point(1,1),4326) FROM generate_series(1,5) n",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db.clone(), c.clone()).unwrap();
    for table in ["big", "small"] {
        let body = format!(
            r#"{{"id":"batch_{table}","title":"X","source":{{"schema":"http_export_batch","table":"{table}"}},"properties":["label"],"access":{{"public":true,"public_download":true}}}}"#
        );
        service.create(body.as_bytes()).await.unwrap();
    }
    let header = r#"{"type":"FeatureCollection","features":["#;
    let (a, server) = serve(service).await;
    let (head, body, decoded) = cut_request(a, "batch_big/export").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(head.contains("transfer-encoding: chunked"));
    assert!(decoded.starts_with(header));
    assert!(!body.contains("bbbb") && !body.contains("]}"));
    assert!(!body.ends_with("0\r\n\r\n"));
    let (head, body, baseline) = admitted(a, "batch_small/export").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(body.ends_with("0\r\n\r\n"));
    assert!(baseline.starts_with(header) && baseline.ends_with("]}"));
    // One-digit ids and equal labels and points: every feature has the same size.
    let size = (baseline.len() - header.len() - 2 - 4) / 5;
    assert_eq!(header.len() + 5 * size + 4 + 2, baseline.len());
    server.abort();
    let two = header.len() + 2 * size + 1;
    c.limits.export_bytes = (two + 2) as u32;
    let (a, server) = serve(Publications::new(db.clone(), c).unwrap()).await;
    let (head, body, decoded) = cut_request(a, "batch_small/export").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert_eq!(decoded, &baseline[..two]);
    assert!(!body.contains("}]}") && !body.ends_with("0\r\n\r\n"));
    assert!(!decoded.ends_with("]}"));
    let (head, _, _) = admitted(a, "batch_small/export").await;
    assert!(head.starts_with("http/1.1 200"), "{head}");
    server.abort();
    db.close().await;
}
