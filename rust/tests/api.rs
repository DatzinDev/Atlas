use atlas::{
    api::{failure, log},
    auth::{http::authenticate, jwt::Verifier},
    config::Config,
    domain::Error,
    migrations::migrate,
    postgis::connection::Database,
    services::publications::Publications,
};
use axum::body::to_bytes;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, get_current_timestamp};
use serde_json::Value;
use std::{
    collections::HashSet,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
};

/// `^[0-9a-f]{16}-[0-9]+$` without a regex dependency.
fn well_formed(id: &str) -> bool {
    id.split_once('-').is_some_and(|(nonce, n)| {
        nonce.len() == 16
            && nonce
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
    })
}

#[tokio::test]
async fn failures_share_one_id_generator() {
    let mut ids = HashSet::new();
    let mut nonces = HashSet::new();
    for error in [Error::NotFound, Error::InvalidToken, Error::QueryTooLarge]
        .into_iter()
        .cycle()
        .take(100)
    {
        let response = failure(error);
        let status = response.status();
        let header = response
            .headers()
            .get("x-request-id")
            .map(|v| v.to_str().unwrap().to_owned());
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = body["request_id"].as_str().unwrap().to_owned();
        assert!(well_formed(&id), "{id}");
        if let Some(header) = header {
            assert_eq!(header, id);
        }
        assert!(status.is_client_error());
        nonces.insert(id.split_once('-').unwrap().0.to_owned());
        assert!(ids.insert(id.clone()), "repeated {id}");
    }
    assert_eq!(ids.len(), 100);
    assert_eq!(nonces.len(), 1, "nonce is fixed per process: {nonces:?}");
}

#[test]
fn log_accepts_any_json_value() {
    log(serde_json::json!({"event": "test", "detail": "line\nbreak"}));
}

/// Every variant with its published `(code, message)`, in `position` order.
#[rustfmt::skip]
const TABLE: [(Error, &str, &str); 15] = [
    (Error::InvalidRequest, "invalid_request", "The request has an invalid or unsupported parameter, header or body."),
    (Error::InvalidFilter, "invalid_filter", "The filter is not a valid predicate for this dataset."),
    (Error::InvalidToken, "invalid_token", "The bearer token is not valid."),
    (Error::PermissionDenied, "permission_denied", "The principal is not allowed to perform this operation."),
    (Error::DatasetNotFound, "dataset_not_found", "The dataset does not exist or is not visible."),
    (Error::NotFound, "not_found", "No route matches this path."),
    (Error::MethodNotAllowed, "method_not_allowed", "The method is not allowed for this route."),
    (Error::PublicationConflict, "publication_conflict", "A publication with this id already exists."),
    (Error::SchemaChanged, "schema_changed", "The publication no longer matches its source."),
    (Error::RevisionMismatch, "revision_mismatch", "The If-Match revision does not match the current revision."),
    (Error::QueryTooLarge, "query_too_large", "The request exceeds a configured size or result budget."),
    (Error::InternalError, "internal_error", "Unexpected server error; report the request_id."),
    (Error::CapacityExceeded, "capacity_exceeded", "The server is at capacity; retry later."),
    (Error::DatabaseUnavailable, "database_unavailable", "The database is unavailable; retry later."),
    (Error::QueryTimeout, "query_timeout", "The query exceeded its time budget."),
];

/// No wildcard: a new variant fails to compile until it gets a `TABLE` row.
#[rustfmt::skip]
fn position(error: Error) -> usize {
    use Error::*;
    match error {
        InvalidRequest => 0, InvalidFilter => 1, InvalidToken => 2, PermissionDenied => 3,
        DatasetNotFound => 4, NotFound => 5, MethodNotAllowed => 6, PublicationConflict => 7,
        SchemaChanged => 8, RevisionMismatch => 9, QueryTooLarge => 10, InternalError => 11,
        CapacityExceeded => 12, DatabaseUnavailable => 13, QueryTimeout => 14,
    }
}

#[tokio::test]
async fn every_error_has_its_fixed_message() {
    let (mut codes, mut messages) = (HashSet::new(), HashSet::new());
    for (i, (error, code, message)) in TABLE.into_iter().enumerate() {
        assert_eq!(position(error), i, "{error:?}");
        let body = to_bytes(failure(error).into_body(), 4096).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], code, "{error:?}");
        assert_eq!(body["message"], message, "{error:?}");
        // Regression: the message used to be the code with spaces.
        assert_ne!(body["message"], code.replace('_', " "), "{error:?}");
        assert!(codes.insert(code) && messages.insert(message), "{error:?}");
    }
    assert_eq!((codes.len(), messages.len()), (15, 15));
}

/// Raw HTTP/1.1 round trip; returns the status and the unparsed body.
async fn call(
    a: SocketAddr,
    method: &str,
    target: &str,
    headers: &str,
    body: &str,
) -> (String, String) {
    let raw = format!(
        "{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
        body.len()
    );
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(a).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        // Bytes, not a String: a 200 vector tile is binary.
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        String::from_utf8_lossy(&response).into_owned()
    })
    .await
    .unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head[9..12].to_owned(), body.to_owned())
}

/// Asserts an error body carries its published message and returns the code.
fn fixed(body: &str) -> String {
    let value: Value = serde_json::from_str(body).unwrap_or_else(|_| panic!("not JSON: {body}"));
    let code = value["code"].as_str().unwrap_or_else(|| panic!("{body}"));
    let row = TABLE.iter().find(|(_, c, _)| *c == code);
    assert_eq!(
        value["message"],
        row.unwrap_or_else(|| panic!("{body}")).2,
        "{body}"
    );
    code.to_owned()
}

async fn server() -> SocketAddr {
    let url = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let mut config = Config::parse("", Some(url), None).unwrap();
    config.allowed_schemas = vec!["api_hints".into()];
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS api_hints CASCADE",
        "CREATE SCHEMA api_hints",
        "CREATE TABLE api_hints.places(id integer PRIMARY KEY, name text, rank int4, geom geometry(Point,4326))",
        "INSERT INTO api_hints.places SELECT n, 'p' || n, n % 3, ST_SetSRID(ST_Point(n, n), 4326) FROM generate_series(1, 9) AS n",
        "DELETE FROM atlas_catalog.datasets WHERE config->'source'->>'schema' <> 'api_hints' OR id LIKE 'hints\\_%'",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db, config).unwrap();
    let body = r#"{"id":"hints_places","title":"T","source":{"schema":"api_hints","table":"places"},"properties":["name","rank"],"access":{"public":true,"public_download":true}}"#;
    service.create(body.as_bytes()).await.unwrap();
    let key = include_bytes!("fixtures/jwt-test-public.pem");
    let verifier = Verifier::new(key, "atlas-test", "atlas").unwrap();
    let app = atlas::api::features::router(service.clone())
        .merge(atlas::api::export::router(service.clone()))
        .merge(atlas::api::tiles::router(service.clone()))
        .merge(atlas::api::datasets::router(service.clone()))
        .merge(atlas::api::admin::router(service, 1024))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(Some(verifier)),
            authenticate,
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

// One server test: a parallel test would race on the api_hints schema and the catalog.
#[tokio::test]
async fn error_messages_over_real_http() {
    let a = server().await;
    no_request_values_echoed(a).await;
    bbox_rejections(a).await;
    query_rules_match_across_routes(a).await;
    query_values_are_decoded(a).await;
}

async fn no_request_values_echoed(a: SocketAddr) {
    let base = "/api/v1/datasets/hints_places";
    let (items, query) = (format!("{base}/items"), format!("{base}/query"));
    // (pre-encoded query value, JSON body value): `secreto` as field, value, op and malformed JSON.
    let filters = [
        (
            "%7B%22op%22%3A%22eq%22%2C%22field%22%3A%22secreto%22%2C%22value%22%3A1%7D",
            r#"{"op":"eq","field":"secreto","value":1}"#,
        ),
        (
            "%7B%22op%22%3A%22eq%22%2C%22field%22%3A%22rank%22%2C%22value%22%3A%22secreto%22%7D",
            r#"{"op":"eq","field":"rank","value":"secreto"}"#,
        ),
        ("%7B%22op%22%3A%22secreto%22%7D", r#"{"op":"secreto"}"#),
        ("%7B%22secreto", r#"{"secreto"#),
    ];
    let get = |target: String| ("GET", target, "", String::new());
    let mut cases = vec![
        get(format!(
            "{base}/export?format=geojson&filter={}",
            filters[0].0
        )),
        get(format!("{items}?secreto=1")),
        get(format!("{items}?sort=secreto:asc")),
        get("/api/v1/datasets/secreto/items".to_owned()),
        (
            "GET",
            items.clone(),
            "Authorization: Bearer secreto\r\n",
            String::new(),
        ),
    ];
    for (encoded, post) in filters {
        cases.push(get(format!("{items}?filter={encoded}")));
        cases.push(("POST", query.clone(), "", format!(r#"{{"filter":{post}}}"#)));
    }
    for (method, target, headers, body) in cases {
        let (status, response) = call(a, method, &target, headers, &body).await;
        let case = format!("{method} {target} {body}: {status} {response}");
        assert!(status.starts_with('4'), "{case}");
        assert!(!response.to_lowercase().contains("secreto"), "{case}");
        let code = fixed(&response);
        match (target.as_str(), headers) {
            ("/api/v1/datasets/secreto/items", _) => assert_eq!(code, "dataset_not_found"),
            (_, h) if !h.is_empty() => assert_eq!(code, "invalid_token"),
            _ => assert!(
                code == "invalid_request" || code == "invalid_filter",
                "{case}"
            ),
        }
    }
}

// Antimeridian, minx = maxx, out-of-range and out-of-CRS-domain bboxes: api_features.rs::bbox_domain.
async fn bbox_rejections(a: SocketAddr) {
    let base = "/api/v1/datasets/hints_places";
    // (query-string form, JSON array contents): 3 numbers, non-numeric, NaN, minx > maxx.
    for (get, post) in [
        ("0,0,1", "0,0,1"),
        ("a,0,1,1", r#""a",0,1,1"#),
        ("NaN,0,1,1", "NaN,0,1,1"),
        ("2,0,1,1", "2,0,1,1"),
    ] {
        let post = format!(r#"{{"bbox":[{post}]}}"#);
        for (method, target, body) in [
            ("GET", format!("{base}/items?bbox={get}"), ""),
            ("POST", format!("{base}/query"), post.as_str()),
            (
                "GET",
                format!("{base}/export?format=geojson&bbox={get}"),
                "",
            ),
        ] {
            let (status, response) = call(a, method, &target, "", body).await;
            assert_eq!(status, "400", "{method} {target} {body}: {response}");
            assert_eq!(
                fixed(&response),
                "invalid_request",
                "{method} {target} {body}"
            );
        }
    }
}

fn bearer(roles: &[&str]) -> String {
    let claims = serde_json::json!({"sub": "alice", "iss": "atlas-test", "aud": "atlas",
        "exp": get_current_timestamp() + 3600, "roles": roles});
    let key = EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwt-test-private.pem")).unwrap();
    let token = encode(&Header::new(Algorithm::RS256), &claims, &key).unwrap();
    format!("Authorization: Bearer {token}\r\n")
}

const BASE: &str = "/api/v1/datasets/hints_places";

/// (path, needs admin, a valid query): every route that reads the query string.
fn routes() -> [(String, bool, &'static str); 8] {
    let admin = "/api/v1/admin/datasets";
    [
        (format!("{BASE}/items"), false, "limit=1"),
        (format!("{BASE}/export"), false, "format=geojson"),
        (format!("{BASE}/tiles/0/0/0.mvt"), false, "properties=name"),
        (format!("{BASE}/tilejson.json"), false, "properties=name"),
        ("/api/v1/datasets".to_owned(), false, "limit=1"),
        (admin.to_owned(), true, "limit=1"),
        // `no_query` routes: no key at all is allowed.
        (BASE.to_owned(), false, ""),
        (format!("{admin}/hints_places"), true, ""),
    ]
}

/// GETs `path?query` and asserts the status and, for errors, the published code.
async fn expect_query(a: SocketAddr, path: &str, admin: bool, query: &str, want: (&str, &str)) {
    let headers = if admin {
        bearer(&["atlas-admin"])
    } else {
        String::new()
    };
    let target = if query.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{query}")
    };
    let (status, body) = call(a, "GET", &target, &headers, "").await;
    assert_eq!(status, want.0, "{target}: {body}");
    if want.0 != "200" {
        assert_eq!(fixed(&body), want.1, "{target}");
    }
}

// One shared parser: the same malformed query gives the same status and code on every route.
async fn query_rules_match_across_routes(a: SocketAddr) {
    let req = ("400", "invalid_request");
    for (path, admin, valid) in routes() {
        expect_query(a, &path, admin, valid, ("200", "")).await;
        for query in ["a", "x=1", "limit=1&limit=2", "filter=%ZZ", "filter=%FF"] {
            expect_query(a, &path, admin, query, req).await;
        }
    }
    // Empty value: only `filter` reports invalid_filter.
    let items = format!("{BASE}/items");
    expect_query(a, &items, false, "filter=", ("400", "invalid_filter")).await;
    expect_query(a, &items, false, "limit=", req).await;
    // `format` exists only on export and only as `geojson`.
    expect_query(a, &format!("{BASE}/export"), false, "format=csv", req).await;
}

// Regression: /datasets and /admin/datasets read limit/offset without decoding them.
async fn query_values_are_decoded(a: SocketAddr) {
    for (path, admin, key) in [
        ("/api/v1/datasets", String::new(), "datasets"),
        (
            "/api/v1/admin/datasets",
            bearer(&["atlas-admin"]),
            "publications",
        ),
    ] {
        let target = format!("{path}?limit=%31");
        let (status, body) = call(a, "GET", &target, &admin, "").await;
        assert_eq!(status, "200", "{target}: {body}");
        let page: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(page[key].as_array().map(Vec::len), Some(1), "{page}");
    }
}
