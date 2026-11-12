use atlas::{
    api::{admin, datasets, export, features::router, tiles},
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

/// Sends the request with `Content-Type: application/json` on POST unless the caller sets one.
async fn call(
    a: SocketAddr,
    method: &str,
    target: &str,
    headers: &str,
    body: &str,
) -> (String, String, Value) {
    parse(call_raw(a, method, target, headers, body).await)
}

/// Like `call`, but returns the raw response body.
async fn call_raw(
    a: SocketAddr,
    method: &str,
    target: &str,
    headers: &str,
    body: &str,
) -> (String, String, String) {
    let headers = if method == "POST" && !headers.to_lowercase().contains("content-type:") {
        format!("{headers}Content-Type: application/json\r\n")
    } else {
        headers.to_owned()
    };
    send_raw(a, method, target, &headers, body).await
}

/// Sends exactly the given headers, without adding a default `Content-Type`.
async fn send(
    a: SocketAddr,
    method: &str,
    target: &str,
    headers: &str,
    body: &str,
) -> (String, String, Value) {
    parse(send_raw(a, method, target, headers, body).await)
}

fn parse((status, head, body): (String, String, String)) -> (String, String, Value) {
    (
        status,
        head,
        // Non-JSON bodies (MVT, chunked streams) are kept as lossy text.
        serde_json::from_str(&body).unwrap_or(Value::String(body)),
    )
}

async fn send_raw(
    a: SocketAddr,
    method: &str,
    target: &str,
    headers: &str,
    body: &str,
) -> (String, String, String) {
    let raw = format!(
        "{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
        body.len()
    );
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(a).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        String::from_utf8_lossy(&response).into_owned()
    })
    .await
    .unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head[9..12].to_owned(), head.to_lowercase(), body.to_owned())
}

fn ids(body: &Value) -> Vec<i64> {
    body["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_i64().unwrap())
        .collect()
}

async fn serve(service: Publications) -> SocketAddr {
    let verifier = Verifier::new(
        include_bytes!("fixtures/jwt-test-public.pem"),
        "atlas-test",
        "atlas",
    )
    .unwrap();
    let admin_body = atlas::config::Limits::default().admin_body_bytes;
    let app = router(service.clone())
        .merge(datasets::router(service.clone()))
        .merge(tiles::router(service.clone()))
        .merge(export::router(service.clone()))
        .merge(admin::router(service, admin_body))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(Some(verifier)),
            authenticate,
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

async fn server() -> (SocketAddr, Database, Config) {
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
        "CREATE TABLE items_test.mercator(id integer PRIMARY KEY, name text, rank int4, geom geometry(Point,3857))",
        "INSERT INTO items_test.mercator SELECT n, 'm' || n, n % 3, ST_Transform(ST_SetSRID(ST_Point(n * 10 - 50, n * 9 - 44), 4326), 3857) FROM generate_series(1, 9) AS n UNION ALL SELECT 10, 'm10', 1, ST_Transform(ST_SetSRID(ST_Point(0, 85), 4326), 3857)",
        "CREATE TABLE items_test.utm(id integer PRIMARY KEY, name text, rank int4, geom geometry(Point,32614))",
        "INSERT INTO items_test.utm SELECT n, 'u' || n, 1, ST_Transform(ST_SetSRID(ST_Point(n - 104.5, 17.5 + n % 7), 4326), 32614) FROM generate_series(1, 9) AS n",
        // Chosen by SQL: inside the densified bbox, outside the straight-edged transform.
        "INSERT INTO items_test.utm SELECT 100, 'edge', 1, ST_PointOnSurface(ST_Difference(ST_Transform(ST_Segmentize(e, 1), 32614), ST_Transform(e, 32614))) FROM ST_MakeEnvelope(-102, 18, -96, 22, 4326) AS e",
        "CREATE FUNCTION items_test.boom(n integer) RETURNS text LANGUAGE plpgsql AS $$BEGIN RETURN 'p' || n; END$$",
        "CREATE VIEW items_test.boom_view AS SELECT id, items_test.boom(id) AS name, rank, geom FROM items_test.places",
        "CREATE TABLE items_test.parcelas(id integer PRIMARY KEY, geom geometry(Point,4326), nombre text, tenant_id int4, payload jsonb)",
        "INSERT INTO items_test.parcelas(id, nombre, tenant_id, geom) SELECT id, nombre, tenant, ST_SetSRID(ST_Point(10 + id * 0.01, 10 + id * 0.01), 4326) FROM (VALUES (1,'seven1',7),(2,'nine2',9),(3,'seven3',7),(4,'nobody4',NULL),(5,'seven5',7),(6,'nine6',9)) AS v(id,nombre,tenant)",
        "DELETE FROM atlas_catalog.datasets WHERE config->'source'->>'schema' <> 'items_test' OR id LIKE 'items\\_%'",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db.clone(), config.clone()).unwrap();
    let both = r#"{"view_roles":["viewer"],"query_roles":["viewer"]}"#;
    let download = r#"{"public":true,"public_download":true}"#;
    for (id, table, access) in [
        ("items_public", "places", r#"{"public":true}"#),
        ("items_doomed", "doomed", r#"{"public":true}"#),
        ("items_view", "places", r#"{"view_roles":["viewer"]}"#),
        ("items_query", "places", both),
        ("items_mercator", "mercator", download),
        ("items_utm", "utm", download),
    ] {
        let body = format!(
            r#"{{"id":"{id}","title":"T","source":{{"schema":"items_test","table":"{table}"}},"properties":["name","rank"],"filter":{{"op":"gte","field":"rank","value":1}},"access":{access}}}"#
        );
        service.create(body.as_bytes()).await.unwrap();
    }
    let boom = r#"{"id":"items_boom","title":"T","source":{"schema":"items_test","table":"boom_view","id_column":"id"},"properties":["name","rank"],"access":{"public":true}}"#;
    service.create(boom.as_bytes()).await.unwrap();
    (serve(service).await, db, config)
}

// R08, R09
#[tokio::test]
async fn items_and_query_over_real_http() {
    let (a, db, config) = server().await;
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

    let huge = format!(
        "POST {query} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n",
        3 << 20
    );
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(a).unwrap();
        stream.write_all(huge.as_bytes()).unwrap();
        stream.write_all(&[b' '; 4096]).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    })
    .await
    .unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let body: Value = serde_json::from_str(body).unwrap();
    let head = head.to_lowercase();
    assert!(head.starts_with("http/1.1 413"), "{head}");
    assert!(head.contains("application/json") && head.contains("no-store"));
    assert!(body["code"] == "query_too_large" && body["request_id"].is_string());
    let exact = format!("{{}}{}", " ".repeat(510));
    assert_eq!(call(a, "POST", query, "", &exact).await.0, "200");

    // Content-Type is checked before the body is read and before the catalog is consulted.
    for headers in [
        "Content-Type: application/json\r\n",
        "Content-Type: Application/JSON\r\n",
        "Content-Type: application/json; charset=utf-8\r\n",
    ] {
        assert_eq!(
            send(a, "POST", query, headers, "{}").await.0,
            "200",
            "{headers}"
        );
    }
    let private = "/api/v1/datasets/items_view/query";
    for headers in [
        "",
        "Content-Type: text/plain\r\n",
        "Content-Type: application/jsonx\r\n",
        "Content-Type: application/json; charset=latin1\r\n",
        "Content-Type: application/json\r\nContent-Type: application/json\r\n",
    ] {
        for target in [query, private] {
            let (status, _, response) = send(a, "POST", target, headers, "{}").await;
            assert_eq!(status, "400", "{headers:?} {target}: {response}");
            assert_eq!(response["code"], "invalid_request", "{headers:?} {target}");
        }
    }

    // Closed Query: null is not an absent field.
    for field in ["filter", "bbox", "properties", "sort", "limit", "offset"] {
        let body = format!(r#"{{"{field}":null}}"#);
        let (status, _, response) = call(a, "POST", query, "", &body).await;
        assert_eq!(status, "400", "{body}: {response}");
        assert_eq!(response["code"], "invalid_request", "{body}");
    }

    bbox_domain(a, &db).await;

    hidden_filter_columns(a, &db).await;

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

    // Regression: privilege/internal errors raised while reading a published view
    // are server faults (500), not an unavailable database (503) or a client error.
    let boom = "/api/v1/datasets/items_boom/items";
    assert_eq!(call(a, "GET", boom, "", "").await.0, "200");
    for code in ["XX000", "42501", "42883"] {
        let mut tx = db.begin_catalog().await.unwrap();
        let replace = format!(
            "CREATE OR REPLACE FUNCTION items_test.boom(n integer) RETURNS text LANGUAGE plpgsql AS $$BEGIN RAISE EXCEPTION 'leaked-detail-{code}' USING ERRCODE = '{code}'; END$$"
        );
        // Safe: `code` only ever comes from the fixed SQLSTATE list above; function bodies take no binds.
        sqlx::query(sqlx::AssertSqlSafe(replace))
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        for (method, target, body) in [
            ("GET", boom.to_owned(), ""),
            ("POST", "/api/v1/datasets/items_boom/query".to_owned(), "{}"),
        ] {
            let (status, _, body) = call(a, method, &target, "", body).await;
            assert_eq!(status, "500", "{code} {method}: {body}");
            assert_eq!(body["code"], "internal_error", "{code}");
            assert!(body["request_id"].is_string() && body.get("features").is_none());
            let text = body.to_string();
            assert!(
                !text.contains(code) && !text.contains("leaked-detail") && !text.contains("boom"),
                "{text}"
            );
        }
    }

    page_byte_budget(&db, config).await;
}

// Called from the single server test: a parallel test would race on the items_test schema.
async fn bbox_domain(a: SocketAddr, db: &Database) {
    let base = "/api/v1/datasets";
    let world = "bbox=-180,-90,180,90";
    for id in ["items_public", "items_mercator"] {
        let (status, _, all) =
            call(a, "GET", &format!("{base}/{id}/items?limit=100"), "", "").await;
        assert_eq!(status, "200", "{id} baseline: {all}");
        let target = format!("{base}/{id}/items?limit=100&{world}");
        let (status, _, body) = call(a, "GET", &target, "", "").await;
        assert_eq!(status, "200", "{id}: {body}");
        assert!(
            !ids(&all).is_empty() && ids(&body) == ids(&all),
            "{id}: {body}"
        );
    }
    let mercator = format!("{base}/items_mercator/query");
    let polygon = |ring: &str| {
        format!(
            r#"{{"filter":{{"op":"intersects","geometry":{{"type":"Polygon","coordinates":[{ring}]}}}},"limit":100}}"#
        )
    };
    for (ring, expected) in [
        (
            "[[-180,0],[180,0],[180,90],[-180,90],[-180,0]]",
            vec![5_i64, 7, 8, 10],
        ),
        ("[[0,86],[1,86],[1,90],[0,90],[0,86]]", vec![]),
    ] {
        let (status, _, body) = call(a, "POST", &mercator, "", &polygon(ring)).await;
        assert_eq!(status, "200", "{ring}: {body}");
        assert_eq!(ids(&body), expected, "{ring}");
    }

    let utm = format!("{base}/items_utm");
    let mut bad = vec![("items_utm", "-180,-90,180,90")];
    for id in ["items_public", "items_mercator", "items_utm"] {
        for bbox in ["170,-10,-170,10", "0,0,0,1", "-181,0,0,1", "0,0,1,91"] {
            bad.push((id, bbox));
        }
    }
    for (id, bbox) in bad {
        let post = format!(r#"{{"bbox":[{bbox}]}}"#);
        for (method, target, body) in [
            ("GET", format!("{base}/{id}/items?bbox={bbox}"), ""),
            ("POST", format!("{base}/{id}/query"), post.as_str()),
            (
                "GET",
                format!("{base}/{id}/export?format=geojson&bbox={bbox}"),
                "",
            ),
        ] {
            let (status, _, response) = call(a, method, &target, "", body).await;
            assert_eq!(status, "400", "{method} {target}: {response}");
            assert_eq!(response["code"], "invalid_request", "{method} {target}");
        }
    }
    let world_filter = polygon("[[-180,-90],[180,-90],[180,90],[-180,90],[-180,-90]]");
    let (status, _, body) = call(a, "POST", &format!("{utm}/query"), "", &world_filter).await;
    assert_eq!(
        (status.as_str(), &body["code"]),
        ("400", &"invalid_filter".into())
    );

    let (status, _, body) = call(
        a,
        "GET",
        &format!("{utm}/items?limit=100&bbox=-102,18,-96,22"),
        "",
        "",
    )
    .await;
    assert_eq!(status, "200", "{body}");
    let mut tx = db.begin_catalog().await.unwrap();
    let oracle: Vec<i32> = sqlx::query_scalar("SELECT id FROM items_test.utm WHERE rank >= 1 AND ST_Intersects(geom, ST_Transform(ST_Segmentize(ST_MakeEnvelope(-102,18,-96,22,4326),1),32614)) ORDER BY id")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(
        oracle.contains(&100),
        "fixture must exercise densification: {oracle:?}"
    );
    let oracle: Vec<i64> = oracle.into_iter().map(i64::from).collect();
    assert_eq!(ids(&body), oracle);
}

// Shares the setup above: tests in this binary run concurrently and the
// setup resets the items_test schema and items_* catalog rows.
async fn page_byte_budget(db: &Database, mut config: Config) {
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "CREATE TABLE items_test.budget(id integer PRIMARY KEY, name text, rank int4, geom geometry(Point,4326))",
        "INSERT INTO items_test.budget SELECT n, repeat('n', 100), n, ST_SetSRID(ST_Point(n, n), 4326) FROM generate_series(1, 10) AS n",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    config.limits.page_bytes = 1000;
    let service = Publications::new(db.clone(), config.clone()).unwrap();
    let dataset = r#"{"id":"items_budget","title":"T","source":{"schema":"items_test","table":"budget"},"properties":["name","rank"],"access":{"public":true}}"#;
    service.create(dataset.as_bytes()).await.unwrap();
    let a = serve(service).await;
    let items = "/api/v1/datasets/items_budget/items";
    let query = "/api/v1/datasets/items_budget/query";

    let rejected = |status: &str, head: &str, body: &str| {
        assert_eq!(status, "413", "{head}");
        assert!(head.contains("application/json") && !head.contains("geo+json"));
        assert!(!body.contains("FeatureCollection") && !body.contains("\"Feature\""));
        let body: Value = serde_json::from_str(body).unwrap();
        assert!(body["code"] == "query_too_large" && body["request_id"].is_string());
    };

    // Regression: ten ~200 byte features no longer fit in a 1000 byte page.
    let (status, head, body) = call_raw(a, "GET", &format!("{items}?limit=10"), "", "").await;
    rejected(&status, &head, &body);
    let (status, head, body) = call_raw(a, "POST", query, "", r#"{"limit":10}"#).await;
    rejected(&status, &head, &body);
    // No explicit limit means default_limit (1000): every row, over budget.
    let (status, head, body) = call_raw(a, "GET", items, "", "").await;
    rejected(&status, &head, &body);

    let (status, head, raw) = call_raw(a, "GET", &format!("{items}?limit=2"), "", "").await;
    assert_eq!(status, "200", "{head}");
    assert!(head.contains("application/geo+json"));
    assert!(raw.len() <= 1000, "{}", raw.len());
    let page: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(ids(&page), [1, 2]);
    assert!(page["numberReturned"] == 2 && page["hasMore"] == true);
    let (status, _, posted) = call_raw(a, "POST", query, "", r#"{"limit":2}"#).await;
    assert_eq!((status.as_str(), posted.as_str()), ("200", raw.as_str()));

    // The budget applies to the bytes actually returned, not to the limit.
    let few = r#"{"limit":10,"filter":{"op":"lte","field":"rank","value":2}}"#;
    let (status, _, body) = call(a, "POST", query, "", few).await;
    assert_eq!(status, "200", "{body}");
    assert!(ids(&body) == [1, 2] && body["hasMore"] == false);
    let (status, _, body) = call(a, "GET", &format!("{items}?limit=10&offset=8"), "", "").await;
    assert_eq!(status, "200", "{body}");
    assert!(ids(&body) == [9, 10] && body["numberReturned"] == 2 && body["hasMore"] == false);

    // Every 200 fits the budget, and once a page is too large so is every longer one.
    let mut too_large = false;
    for limit in 1..=10 {
        let (status, head, body) =
            call_raw(a, "GET", &format!("{items}?limit={limit}"), "", "").await;
        if status == "413" {
            rejected(&status, &head, &body);
            too_large = true;
            continue;
        }
        assert!(!too_large && status == "200", "limit={limit}: {status}");
        assert!(body.len() <= 1000, "limit={limit}: {}", body.len());
        let page: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(ids(&page), (1..=limit).collect::<Vec<i64>>());
        assert!(page["numberReturned"] == limit && page["hasMore"] == true);
    }
    assert!(too_large);

    // The bound is inclusive and ignores the look-ahead row used for hasMore.
    config.limits.page_bytes = raw.len() as u32;
    let exact = serve(Publications::new(db.clone(), config.clone()).unwrap()).await;
    let (status, _, body) = call_raw(exact, "GET", &format!("{items}?limit=2"), "", "").await;
    assert_eq!((status.as_str(), body.as_str()), ("200", raw.as_str()));
    config.limits.page_bytes -= 1;
    let short = serve(Publications::new(db.clone(), config).unwrap()).await;
    let (status, head, body) = call_raw(short, "GET", &format!("{items}?limit=2"), "", "").await;
    rejected(&status, &head, &body);
    let (status, head, body) = call_raw(short, "POST", query, "", r#"{"limit":2}"#).await;
    rejected(&status, &head, &body);
}

async fn hidden_filter_columns(a: SocketAddr, db: &Database) {
    let admin = bearer(&["atlas-admin"]);
    let admin = admin.as_str();
    let publications = "/api/v1/admin/datasets";
    let publish = |id: &str, filter: &str| {
        format!(
            r#"{{"id":"{id}","title":"Parcelas","source":{{"schema":"items_test","table":"parcelas"}},"properties":["nombre"],"filter":{filter},"access":{{"public":true,"public_download":true}}}}"#
        )
    };
    let body = publish(
        "items_parcelas",
        r#"{"op":"eq","field":"tenant_id","value":7}"#,
    );
    let (status, _, created) = call(a, "POST", publications, admin, &body).await;
    assert_eq!(status, "201", "{created}");
    assert_eq!(created["metadata"]["attributes"][0]["name"], "nombre");
    assert!(!created["metadata"].to_string().contains("tenant_id"));
    let (status, _, detail) = call(a, "GET", "/api/v1/datasets/items_parcelas", "", "").await;
    assert_eq!(status, "200", "{detail}");
    assert!(!detail.to_string().contains("tenant_id"));

    let items = "/api/v1/datasets/items_parcelas/items";
    let query = "/api/v1/datasets/items_parcelas/query";
    for (method, target, body) in [("GET", items, ""), ("POST", query, "{}")] {
        let (status, _, page) = call(a, method, target, "", body).await;
        assert_eq!(status, "200", "{page}");
        assert_eq!(ids(&page), [1, 3, 5]);
        assert!(!page.to_string().contains("tenant_id"));
    }
    // ponytail: needles are matched on the raw (possibly chunked) body.
    for target in [
        "/api/v1/datasets/items_parcelas/tiles/4/8/7.mvt",
        "/api/v1/datasets/items_parcelas/export?format=geojson",
    ] {
        let (status, _, body) = call(a, "GET", target, "", "").await;
        let body = body.to_string();
        assert!(status == "200" && body.contains("seven1"), "{target}");
        for leak in ["tenant_id", "nine2", "nine6"] {
            assert!(!body.contains(leak), "{target}: {leak}");
        }
    }
    let tilejson = "/api/v1/datasets/items_parcelas/tilejson.json";
    let (_, _, document) = call(a, "GET", tilejson, "", "").await;
    let fields = document["vector_layers"][0]["fields"].as_object().unwrap();
    assert_eq!(fields.keys().collect::<Vec<_>>(), ["nombre"]);

    let user = r#"{"filter":{"op":"eq","field":"tenant_id","value":9}}"#;
    let (properties, sort) = (
        format!("{items}?properties=tenant_id"),
        format!("{items}?sort=tenant_id:asc"),
    );
    let missing = publish("items_bad", r#"{"op":"eq","field":"missing","value":7}"#);
    let jsonb = publish("items_bad", r#"{"op":"is_null","field":"payload"}"#);
    for (method, target, headers, body, code) in [
        ("POST", query, "", user, "invalid_filter"),
        ("GET", &*properties, "", "", "invalid_request"),
        ("GET", &*sort, "", "", "invalid_request"),
        ("POST", publications, admin, &*missing, "invalid_request"),
        ("POST", publications, admin, &*jsonb, "invalid_request"),
    ] {
        let (status, _, response) = call(a, method, target, headers, body).await;
        let result = (status.as_str(), response["code"].as_str());
        assert_eq!(result, ("400", Some(code)), "{target} {body}");
    }

    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("ALTER TABLE items_test.parcelas DROP COLUMN tenant_id")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (status, _, body) = call(a, "GET", items, "", "").await;
    assert_eq!(
        (status.as_str(), body["code"].as_str()),
        ("409", Some("schema_changed"))
    );
}
