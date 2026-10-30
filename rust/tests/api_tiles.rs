use atlas::{
    api::tiles::router,
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

fn bearer(roles: &[&str]) -> String {
    let claims = serde_json::json!({"sub": "alice", "iss": "atlas-test", "aud": "atlas",
        "exp": get_current_timestamp() + 3600, "roles": roles});
    let key = EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwt-test-private.pem")).unwrap();
    let token = encode(&Header::new(Algorithm::RS256), &claims, &key).unwrap();
    format!("Authorization: Bearer {token}\r\n")
}

async fn get(address: SocketAddr, target: &str, headers: &str) -> (String, String, Vec<u8>) {
    let raw =
        format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{headers}\r\n");
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        response
    })
    .await
    .unwrap();
    let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&response[..split]).to_lowercase();
    (head[9..12].to_owned(), head, response[split + 4..].to_vec())
}

async fn server() -> SocketAddr {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["tiles_test".into()];
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS tiles_test CASCADE",
        "CREATE SCHEMA tiles_test",
        "CREATE TABLE tiles_test.points(id integer PRIMARY KEY, name text, rank int4, secret text, geom geometry(Point,4326))",
        "INSERT INTO tiles_test.points VALUES (1,'a',1,'x',ST_SetSRID(ST_Point(10,10),4326)), (2,'b',2,'x',ST_SetSRID(ST_Point(10.1,10.1),4326))",
        "DELETE FROM atlas_catalog.datasets WHERE config->'source'->>'schema' <> 'tiles_test' OR id LIKE 'tiles\\_%'",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db, config).unwrap();
    for (id, access) in [
        ("tiles_public", r#"{"public":true}"#),
        (
            "tiles_private",
            r#"{"view_roles":["viewer"],"query_roles":["viewer"]}"#,
        ),
    ] {
        let body = format!(
            r#"{{"id":"{id}","title":"T","source":{{"schema":"tiles_test","table":"points"}},"properties":["name","rank"],"filter":{{"op":"gte","field":"rank","value":2}},"access":{access}}}"#
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
    address
}

#[tokio::test]
async fn vector_tiles_over_real_http() {
    let a = server().await;
    let viewer = bearer(&["viewer"]);
    let public = "/api/v1/datasets/tiles_public/tiles/4/8/7.mvt";
    let (status, head, full) = get(a, public, "").await;
    assert_eq!(status, "200", "{head}");
    assert!(head.contains("content-type: application/vnd.mapbox-vector-tile"));
    assert!(head.contains("cache-control: no-store") && !full.is_empty());
    assert!(!full.windows(6).any(|w| w == b"secret"));
    let only = get(a, &format!("{public}?properties=name"), "").await;
    assert_eq!(only.0, "200");
    assert!(only.2.len() < full.len());
    let none = "%7B%22op%22%3A%22eq%22%2C%22field%22%3A%22name%22%2C%22value%22%3A%22a%22%7D";
    let emptied = get(a, &format!("{public}?filter={none}"), "").await;
    assert_eq!((emptied.0.as_str(), emptied.2.len()), ("200", 0));
    let empty = get(a, "/api/v1/datasets/tiles_public/tiles/4/0/0.mvt", "").await;
    assert_eq!((empty.0.as_str(), empty.2.len()), ("200", 0));
    let private = "/api/v1/datasets/tiles_private/tiles/4/8/7.mvt";
    assert_eq!(get(a, private, "").await.0, "404");
    assert_eq!(get(a, private, &viewer).await.0, "200");

    let missing = "/api/v1/datasets/tiles_none/tiles";
    for target in [
        format!("{missing}/23/0/0.mvt"),
        format!("{missing}/4/16/0.mvt"),
        format!("{missing}/4/0/0.png"),
        format!("{missing}/4/x/0.mvt"),
        format!("{missing}/-1/0/0.mvt"),
        format!("{missing}/4/0/0.mvt?bbox=0,0,1,1"),
        format!("{missing}/4/0/0.mvt?filter=%7B%7D&filter=%7B%7D"),
        format!("{missing}/4/0/0.mvt?filter=%ZZ"),
        format!("{missing}/4/0/0.mvt?properties=name,"),
        "/api/v1/datasets/Bad/tiles/4/0/0.mvt".into(),
    ] {
        let (status, _, body) = get(a, &target, "").await;
        assert_eq!(
            status,
            "400",
            "{target}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    assert_eq!(
        get(a, &format!("{public}?properties=secret"), "").await.0,
        "400"
    );
    assert_eq!(
        get(a, &format!("{public}?filter=%7B%22op%22%3A%22x%22%7D"), "")
            .await
            .0,
        "400"
    );
    assert_eq!(
        get(a, "/api/v1/datasets/tiles_none/tiles/4/8/7.mvt", "")
            .await
            .0,
        "404"
    );
}
