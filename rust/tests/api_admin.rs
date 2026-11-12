use atlas::{
    api::{admin::router, datasets},
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

/// Adds the default JSON media type for POST/PATCH unless the caller sets a Content-Type.
fn typed(method: &str, headers: &str) -> String {
    let media = match method {
        "POST" => "application/json",
        "PATCH" => "application/merge-patch+json",
        _ => return headers.to_owned(),
    };
    if headers.to_lowercase().contains("content-type:") {
        headers.to_owned()
    } else {
        format!("{headers}Content-Type: {media}\r\n")
    }
}

/// Body of a raw HTTP response as JSON.
fn json(response: &str) -> serde_json::Value {
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

async fn expect(
    address: SocketAddr,
    request: (&str, &str, &str, &str),
    status: &str,
    needle: &str,
) -> String {
    let (method, target, headers, body) = request;
    let headers = typed(method, headers);
    expect_raw(address, (method, target, &headers, body), status, needle).await
}

async fn send(address: SocketAddr, request: (&str, &str, &str, &str)) -> String {
    let (method, target, headers, body) = request;
    let raw = format!(
        "{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
        body.len()
    );
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    })
    .await
    .unwrap()
}

/// GETs `target` and returns the JSON body after asserting a 200.
async fn listed(address: SocketAddr, target: &str, headers: &str) -> serde_json::Value {
    let response = send(address, ("GET", target, headers, "")).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{target}: {response}");
    let (_, body) = response.split_once("\r\n\r\n").unwrap();
    serde_json::from_str(body).unwrap()
}

fn ids(page: &serde_json::Value, key: &str) -> Vec<String> {
    page[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .collect()
}

/// Like `expect`, but sends exactly the given headers.
async fn expect_raw(
    address: SocketAddr,
    request: (&str, &str, &str, &str),
    status: &str,
    needle: &str,
) -> String {
    let (method, target, _, _) = request;
    let response = send(address, request).await;
    let context = format!("{method} {target}: {response}");
    assert!(
        response.starts_with(&format!("HTTP/1.1 {status}")),
        "{context}"
    );
    assert!(response.to_lowercase().contains(needle), "{context}");
    if status.starts_with('4') {
        assert!(response.contains("request_id") && !response.contains("SELECT"));
    }
    response
}

async fn declared(address: SocketAddr, method: &str, target: &str, headers: &str) -> String {
    let headers = typed(method, headers);
    let raw = format!(
        "{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n",
        3 << 20
    );
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        stream.write_all(&[b' '; 4096]).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response.to_lowercase()
    })
    .await
    .unwrap()
}

fn settings(schema: &str) -> Config {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec![schema.into()];
    config
}

async fn server() -> (SocketAddr, Database) {
    let config = settings("admin_test");
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
    (serve(config, db.clone()).await, db)
}

/// Serves the admin and public routers; a second call over the same database is a restart.
async fn serve(config: Config, db: Database) -> SocketAddr {
    let verifier = Verifier::new(
        include_bytes!("fixtures/jwt-test-public.pem"),
        "atlas-test",
        "atlas",
    )
    .unwrap();
    let service = Publications::new(db, config).unwrap();
    let app = router(service.clone(), 1024)
        .merge(datasets::router(service))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(Some(verifier)),
            authenticate,
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

#[tokio::test]
async fn admin_routes_over_real_http() {
    let (a, db) = server().await;
    let (admin, viewer) = (bearer(&["atlas-admin"]), bearer(&["viewer"]));
    let revision = |n: &str| format!("{admin}If-Match: {n}\r\n");
    let large = BODY.replace("Roads", &"x".repeat(1024));
    let unknown = BODY.replace("\"properties\"", "\"extra\":1,\"properties\"");
    let title = r#"{"title":"New"}"#;
    let polygon = serde_json::json!({"op":"within", "geometry":{"type":"Polygon",
        "coordinates":[[[0,0],[2,0],[2,2],[0,2],[0,0]]]}});
    let mut published: serde_json::Value = serde_json::from_str(BODY).unwrap();
    published["filter"] = serde_json::json!({"op":"and", "args":[
        {"op":"eq","field":"name","value":"a"}, polygon.clone()]});
    let published = published.to_string();
    expect(a, ("POST", LIST, "", BODY), "403", "permission_denied").await;
    expect(a, ("POST", LIST, &viewer, BODY), "403", "permission_denied").await;
    expect(a, ("POST", LIST, &admin, &published), "201", "etag: \"1\"").await;
    expect(
        a,
        ("POST", LIST, &admin, BODY),
        "409",
        "publication_conflict",
    )
    .await;
    expect(
        a,
        ("POST", LIST, &admin, &unknown),
        "400",
        "invalid_request",
    )
    .await;
    expect(a, ("POST", LIST, &admin, &large), "413", "query_too_large").await;
    for (method, target, headers) in [
        ("POST", LIST, admin.clone()),
        ("PATCH", ITEM, revision("\"1\"")),
    ] {
        let response = declared(a, method, target, &headers).await;
        assert!(response.starts_with("http/1.1 413"), "{response}");
        for needle in [
            "application/json",
            "no-store",
            "query_too_large",
            "request_id",
        ] {
            assert!(response.contains(needle), "{needle}: {response}");
        }
    }
    let response = declared(a, "POST", LIST, &viewer).await;
    assert!(response.starts_with("http/1.1 403"), "{response}");
    let exact = format!("{BODY}{}", " ".repeat(1024 - BODY.len()));
    let exact = exact.replace("api_roads", "api_exact");
    expect(a, ("POST", LIST, &admin, &exact), "201", "etag").await;
    let doomed = ("DELETE", "/api/v1/admin/datasets/api_exact");
    expect(
        a,
        (doomed.0, doomed.1, &revision("\"1\""), ""),
        "204",
        "no-store",
    )
    .await;
    let ct = BODY.replace("api_roads", "api_ct");
    let ct_item = "/api/v1/admin/datasets/api_ct";
    let text = "Content-Type: text/plain\r\n";
    let headers = format!("{viewer}{text}");
    expect_raw(a, ("POST", LIST, &headers, &ct), "403", "permission_denied").await;
    for media in [
        "",
        "Content-Type: text/plain\r\n",
        "Content-Type: application/jsonx\r\n",
        "Content-Type: application/json; charset=latin1\r\n",
        "Content-Type: application/json\r\nContent-Type: application/json\r\n",
    ] {
        let headers = format!("{admin}{media}");
        expect_raw(a, ("POST", LIST, &headers, &ct), "400", "invalid_request").await;
    }
    for media in [
        "Content-Type: Application/JSON\r\n",
        "Content-Type: application/json; charset=utf-8\r\n",
    ] {
        let headers = format!("{admin}{media}");
        expect_raw(a, ("POST", LIST, &headers, &ct), "201", "etag: \"1\"").await;
        expect(
            a,
            ("DELETE", ct_item, &revision("\"1\""), ""),
            "204",
            "no-store",
        )
        .await;
    }
    let mut nulls = Vec::new();
    for field in ["description", "filter", "access"] {
        let mut doc: serde_json::Value = serde_json::from_str(&ct).unwrap();
        doc[field] = serde_json::Value::Null;
        nulls.push(doc);
    }
    let mut doc: serde_json::Value = serde_json::from_str(&ct).unwrap();
    doc["source"]["geometry_column"] = serde_json::Value::Null;
    nulls.push(doc);
    for doc in nulls {
        let doc = doc.to_string();
        expect(a, ("POST", LIST, &admin, &doc), "400", "invalid_request").await;
    }
    expect(a, ("GET", ct_item, &admin, ""), "404", "dataset_not_found").await;
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
    let item = format!("{ITEM}?unknown=1");
    let other = BODY.replace("api_roads", "api_other");
    for (method, target, headers, body) in [
        ("GET", item.as_str(), admin.clone(), ""),
        (
            "GET",
            "/api/v1/admin/datasets/api_no?unknown=1",
            admin.clone(),
            "",
        ),
        ("POST", &paged("unknown=1"), admin.clone(), &other),
        ("PATCH", &item, revision("\"1\""), title),
        ("DELETE", &item, revision("\"1\""), ""),
    ] {
        expect(
            a,
            (method, target, &headers, body),
            "400",
            "invalid_request",
        )
        .await;
    }
    expect(
        a,
        ("GET", "/api/v1/admin/datasets/api_other", &admin, ""),
        "404",
        "dataset_not_found",
    )
    .await;
    expect(a, ("PATCH", ITEM, &admin, title), "400", "invalid_request").await;
    expect(
        a,
        ("PATCH", ITEM, &revision("W/\"1\""), title),
        "400",
        "invalid_request",
    )
    .await;
    expect(
        a,
        ("PATCH", ITEM, &revision("\"9\""), title),
        "412",
        "revision_mismatch",
    )
    .await;
    let headers = format!("{viewer}If-Match: \"1\"\r\n{text}");
    expect_raw(
        a,
        ("PATCH", ITEM, &headers, title),
        "403",
        "permission_denied",
    )
    .await;
    for media in [
        "Content-Type: text/plain\r\n",
        "Content-Type: application/merge-patch+json\r\nContent-Type: application/json\r\n",
    ] {
        let headers = format!("{}{media}", revision("\"1\""));
        expect_raw(
            a,
            ("PATCH", ITEM, &headers, title),
            "400",
            "invalid_request",
        )
        .await;
    }
    expect(
        a,
        ("PATCH", ITEM, &revision("\"1\""), title),
        "200",
        "etag: \"2\"",
    )
    .await;
    expect(
        a,
        ("DELETE", ITEM, &revision("\"1\""), ""),
        "412",
        "revision_mismatch",
    )
    .await;
    let spatial_patch = serde_json::json!({"filter":polygon}).to_string();
    let as_json = format!(
        "{}Content-Type: Application/JSON; charset=utf-8\r\n",
        revision("\"2\"")
    );
    expect(
        a,
        ("PATCH", ITEM, &as_json, &spatial_patch),
        "200",
        "etag: \"3\"",
    )
    .await;
    let invalid = r#"{"filter":{"op":"within","geometry":{"type":"Polygon","coordinates":[[[0,0],[2,2],[0,2],[2,0],[0,0]]]}}}"#;
    expect(
        a,
        ("PATCH", ITEM, &revision("\"3\""), invalid),
        "400",
        "invalid_filter",
    )
    .await;
    // A fixed filter on a column that does not exist is invalid_request.
    let missing = r#"{"filter":{"op":"is_null","field":"secret"}}"#;
    expect(
        a,
        ("PATCH", ITEM, &revision("\"3\""), missing),
        "400",
        "invalid_request",
    )
    .await;
    let described = r#"{"description":"D"}"#;
    expect(
        a,
        ("PATCH", ITEM, &revision("\"3\""), described),
        "200",
        "\"description\":\"d\"",
    )
    .await;
    // In Merge Patch, null keeps its meaning: it removes the optional field.
    let cleared = r#"{"description":null}"#;
    let response = expect(
        a,
        ("PATCH", ITEM, &revision("\"4\""), cleared),
        "200",
        "etag: \"5\"",
    )
    .await;
    assert!(
        json(&response)["config"]["description"].is_null(),
        "{response}"
    );
    expect(
        a,
        ("DELETE", ITEM, &revision("\"5\""), ""),
        "204",
        "cache-control: no-store",
    )
    .await;
    expect(a, ("GET", ITEM, &admin, ""), "404", "dataset_not_found").await;
    let roles = BODY.replace("api_roads", "api_roles");
    let roles_item = "/api/v1/admin/datasets/api_roles";
    expect(a, ("POST", LIST, &admin, &roles), "201", "etag: \"1\"").await;
    for key in ["view_roles", "query_roles", "download_roles"] {
        let access = format!(r#""access":{{"{key}":[" viewer"]}}"#);
        let posted = BODY.replace("\"properties\"", &format!("{access},\"properties\""));
        let patch = format!("{{{access}}}");
        expect(a, ("POST", LIST, &admin, &posted), "400", "invalid_request").await;
        expect(
            a,
            ("PATCH", roles_item, &revision("\"1\""), &patch),
            "400",
            "invalid_request",
        )
        .await;
        for stored in [r#"[" viewer"]"#, "[]"] {
            let mut tx = db.begin_catalog().await.unwrap();
            sqlx::query("UPDATE atlas_catalog.datasets SET config=jsonb_set(config,ARRAY['access',$1],$2::jsonb) WHERE id='api_roles'")
                .bind(key)
                .bind(stored)
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            if stored != "[]" {
                expect(
                    a,
                    ("PATCH", roles_item, &revision("\"1\""), title),
                    "409",
                    "schema_changed",
                )
                .await;
            }
        }
    }
    expect(
        a,
        ("DELETE", roles_item, &revision("\"1\""), ""),
        "204",
        "no-store",
    )
    .await;

    // Rows that no longer validate are listed in place instead of blocking the page.
    let edit = |statement: &'static str| {
        let db = db.clone();
        async move {
            let mut tx = db.begin_catalog().await.unwrap();
            sqlx::query(statement).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
        }
    };
    let public = |id: &str| {
        BODY.replace("api_roads", id)
            .replace("\"properties\"", r#""access":{"public":true},"properties""#)
    };
    for id in ["api_a", "api_c"] {
        expect(a, ("POST", LIST, &admin, &public(id)), "201", "etag: \"1\"").await;
    }
    edit(r#"INSERT INTO atlas_catalog.datasets(id,config,metadata) SELECT 'api_b', jsonb_set(jsonb_set(config,'{id}','"api_b"'),'{source,schema}','"retirado"'), metadata FROM atlas_catalog.datasets WHERE id='api_a'"#).await;
    let marked = |id: &str, revision: i64| serde_json::json!({"id": id, "revision": revision, "error": "schema_changed"});
    let page = listed(a, LIST, &admin).await;
    assert_eq!(ids(&page, "publications"), ["api_a", "api_b", "api_c"]);
    assert_eq!(page["hasMore"], false);
    let entries = page["publications"].as_array().unwrap();
    assert_eq!(entries[1], marked("api_b", 1));
    for entry in [&entries[0], &entries[2]] {
        assert!(entry["config"].is_object() && entry["metadata"].is_object());
        assert!(entry.get("error").is_none(), "{entry}");
    }
    // Order and hasMore are unchanged around the invalid row.
    let middle = listed(a, &paged("limit=1&offset=1"), &admin).await;
    assert_eq!(
        middle["publications"],
        serde_json::json!([marked("api_b", 1)])
    );
    assert_eq!(middle["hasMore"], true);
    // Public routes never expose it.
    let shown = listed(a, "/api/v1/datasets", &viewer).await;
    assert_eq!(ids(&shown, "datasets"), ["api_a", "api_c"]);
    expect(
        a,
        ("GET", "/api/v1/datasets/api_a", &viewer, ""),
        "200",
        "api_a",
    )
    .await;
    for headers in [&viewer, &admin] {
        expect(
            a,
            ("GET", "/api/v1/datasets/api_b", headers, ""),
            "404",
            "dataset_not_found",
        )
        .await;
    }
    // Its detail is a conflict that still reveals the revision DELETE needs.
    let broken = "/api/v1/admin/datasets/api_b";
    expect(a, ("GET", broken, &admin, ""), "409", "schema_changed").await;
    expect(
        a,
        ("PATCH", broken, &revision("\"1\""), title),
        "409",
        "schema_changed",
    )
    .await;
    expect(
        a,
        ("DELETE", broken, &revision("\"2\""), ""),
        "412",
        "revision_mismatch",
    )
    .await;
    expect(a, ("GET", broken, &admin, ""), "409", "etag: \"1\"").await;
    expect(
        a,
        ("DELETE", broken, &revision("\"1\""), ""),
        "204",
        "no-store",
    )
    .await;
    expect(a, ("GET", broken, &admin, ""), "404", "dataset_not_found").await;
    // Restarting without the source schema marks its rows; they are removed over the API.
    let b = serve(settings("admin_spare"), db.clone()).await;
    let page = listed(b, LIST, &admin).await;
    assert_eq!(
        page["publications"],
        serde_json::json!([marked("api_a", 1), marked("api_c", 1)])
    );
    assert_eq!(page["hasMore"], false);
    let shown = listed(b, "/api/v1/datasets", &viewer).await;
    assert_eq!(shown["datasets"], serde_json::json!([]));
    let target = format!("{LIST}/api_a");
    expect(b, ("GET", &target, &admin, ""), "409", "etag: \"1\"").await;
    expect(
        b,
        ("DELETE", &target, &revision("\"1\""), ""),
        "204",
        "no-store",
    )
    .await;
    assert_eq!(
        ids(&listed(b, LIST, &admin).await, "publications"),
        ["api_c"]
    );
}
