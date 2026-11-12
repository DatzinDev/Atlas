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

async fn server() -> (SocketAddr, Database) {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["tiles_test".into()];
    config.public_url = Some("https://atlas.example.test/base".into());
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS tiles_test CASCADE",
        "CREATE SCHEMA tiles_test",
        "CREATE TABLE tiles_test.points(id integer PRIMARY KEY, name text, rank int4, secret text, geom geometry(Point,4326))",
        "INSERT INTO tiles_test.points VALUES (1,'a',1,'x',ST_SetSRID(ST_Point(10,10),4326)), (2,'b',2,'x',ST_SetSRID(ST_Point(10.1,10.1),4326)), (3,'a b',3,'x',ST_SetSRID(ST_Point(10.05,10.05),4326))",
        "CREATE TABLE tiles_test.polar(id integer PRIMARY KEY, name text, geom geometry(Geometry,4326))",
        "INSERT INTO tiles_test.polar VALUES (1,'antarctica',ST_GeomFromText('POLYGON((-180 -90,180 -90,180 -60,-180 -60,-180 -90))',4326)), (2,'pole',ST_GeomFromText('POLYGON((0 89,1 89,1 90,0 90,0 89))',4326))",
        "CREATE TABLE tiles_test.doomed AS TABLE tiles_test.points",
        "ALTER TABLE tiles_test.doomed ADD PRIMARY KEY (id)",
        "DELETE FROM atlas_catalog.datasets WHERE config->'source'->>'schema' <> 'tiles_test' OR id LIKE 'tiles\\_%'",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db.clone(), config).unwrap();
    for (id, table, access) in [
        ("tiles_public", "points", r#"{"public":true}"#),
        ("tiles_doomed", "doomed", r#"{"public":true}"#),
        (
            "tiles_private",
            "points",
            r#"{"view_roles":["viewer"],"query_roles":["viewer"]}"#,
        ),
    ] {
        let body = format!(
            r#"{{"id":"{id}","title":"T","source":{{"schema":"tiles_test","table":"{table}"}},"properties":["name","rank"],"filter":{{"op":"gte","field":"rank","value":2}},"access":{access}}}"#
        );
        service.create(body.as_bytes()).await.unwrap();
    }
    let polar = r#"{"id":"tiles_polar","title":"P","source":{"schema":"tiles_test","table":"polar"},"properties":["name"],"access":{"public":true}}"#;
    service.create(polar.as_bytes()).await.unwrap();
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

// R04, R10
#[tokio::test]
async fn vector_tiles_over_real_http() {
    let (a, db) = server().await;
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
    // Regression: a vertex at latitude +-90 used to answer 503 on every tile touching it.
    for target in [
        "/api/v1/datasets/tiles_polar/tiles/0/0/0.mvt",
        "/api/v1/datasets/tiles_polar/tiles/1/0/1.mvt",
    ] {
        let (status, head, body) = get(a, target, "").await;
        assert_eq!(status, "200", "{target}: {head}");
        assert!(
            head.contains("content-type: application/vnd.mapbox-vector-tile")
                && body.windows(10).any(|w| w == b"antarctica")
                && !body.windows(4).any(|w| w == b"pole"),
            "{target}"
        );
    }
    let private = "/api/v1/datasets/tiles_private/tiles/4/8/7.mvt";
    assert_eq!(get(a, private, "").await.0, "404");
    assert_eq!(get(a, private, &viewer).await.0, "200");

    let dir = "/api/v1/datasets/tiles_none/tiles";
    let tile = format!("{dir}/4/0/0.mvt");
    let signed = "%22op%22%3A%22is_null%22%2C%22field%22%3A%22name%22%7D";
    let hidden = "%7B%22op%22%3A%22is_null%22%2C%22field%22%3A%22secret%22%7D";
    let (req, bad, gone) = ("invalid_request", "invalid_filter", "dataset_not_found");
    for (target, code) in [
        (format!("{dir}/23/0/0.mvt"), req),
        (format!("{dir}/4/16/0.mvt"), req),
        (format!("{dir}/4/0/0.png"), req),
        (format!("{dir}/4/x/0.mvt"), req),
        (format!("{dir}/-1/0/0.mvt"), req),
        (format!("{tile}?bbox=0,0,1,1"), req),
        (format!("{tile}?filter=%7B%7D&filter=%7B%7D"), req),
        (format!("{tile}?filter=%ZZ"), req),
        (format!("{tile}?properties=name,"), req),
        (format!("{tile}?filter=%7B%+9{signed}"), req),
        (format!("{tile}?properties=%+9name"), req),
        (format!("{tile}?properties=%-1"), req),
        ("/api/v1/datasets/Bad/tiles/4/0/0.mvt".into(), req),
        (format!("{public}?properties=secret"), req),
        (format!("{public}?filter=%7B%22op%22%3A%22x%22%7D"), bad),
        (format!("{tile}?filter=%7B"), bad),
        (format!("{tile}?filter=%7B%22op%22%3A%22bogus%22%7D"), bad),
        (format!("{tile}?filter={hidden}"), gone),
        (format!("{dir}/4/8/7.mvt"), gone),
    ] {
        let (status, _, body) = get(a, &target, "").await;
        let expected = if code == gone { "404" } else { "400" };
        let found = String::from_utf8_lossy(&body).contains(code);
        assert!(status == expected && found, "{target}");
    }

    let tilejson = "/api/v1/datasets/tiles_public/tilejson.json";
    let (status, head, body) = get(a, tilejson, "X-Forwarded-Host: evil.test\r\n").await;
    assert!(
        status == "200" && head.contains("cache-control: no-store"),
        "{head}"
    );
    let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let url = "https://atlas.example.test/base/api/v1/datasets/tiles_public/tiles/{z}/{x}/{y}.mvt";
    assert_eq!(
        (doc["tilejson"].as_str(), doc["tiles"][0].as_str()),
        (Some("3.0.0"), Some(url))
    );
    assert_eq!(
        (doc["minzoom"].as_u64(), doc["maxzoom"].as_u64()),
        (Some(0), Some(22))
    );
    let layer = &doc["vector_layers"][0];
    assert_eq!(layer["id"], "tiles_public");
    assert_eq!(
        layer["fields"],
        serde_json::json!({"name": "String", "rank": "Number"})
    );
    assert!(
        !String::from_utf8_lossy(&body).contains("evil")
            && !body.windows(6).any(|w| w == b"secret")
    );
    let private = "/api/v1/datasets/tiles_private/tilejson.json";
    assert_eq!(get(a, private, "").await.0, "404");
    assert_eq!(get(a, private, &viewer).await.0, "200");
    assert_eq!(get(a, &format!("{tilejson}?x=1"), "").await.0, "400");

    // TileJSON of a filtered, projected layer: canonical template and narrowed fields.
    let wanted = "%7B%22op%22%3A%22eq%22%2C%22field%22%3A%22name%22%2C%22value%22%3A%22a%20b%22%7D";
    let (status, head, body) = get(
        a,
        &format!("{tilejson}?filter={wanted}&properties=name"),
        "",
    )
    .await;
    assert_eq!(status, "200", "{head}");
    let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let template = doc["tiles"][0].as_str().unwrap().to_owned();
    assert!(
        template.ends_with(&format!(".mvt?filter={wanted}&properties=name")),
        "{template}"
    );
    assert_eq!(template, format!("{url}?filter={wanted}&properties=name"));
    assert_eq!(
        doc["vector_layers"][0]["fields"],
        serde_json::json!({"name": "String"})
    );
    // The template, filled with z/x/y, serves the same features as the tile route.
    let concrete = template
        .strip_prefix("https://atlas.example.test/base")
        .unwrap()
        .replace("{z}/{x}/{y}", "4/8/7");
    let (status, head, via_template) = get(a, &concrete, "").await;
    assert_eq!(status, "200", "{concrete}: {head}");
    let direct = get(a, &format!("{public}?filter={wanted}&properties=name"), "").await;
    assert_eq!(direct.0, "200");
    assert!(!via_template.is_empty() && via_template == direct.2);
    assert!(via_template.windows(3).any(|w| w == b"a b"));
    assert!(!via_template.windows(4).any(|w| w == b"rank") && via_template.len() < full.len());

    // Input spelling does not leak into the template: `+`, lowercase hex and key order are
    // normalised, and the properties separator is percent-encoded.
    let loose = "%7b%22op%22%3a%22eq%22%2c%22field%22%3a%22name%22%2c%22value%22%3a%22a+b%22%7d";
    for (query, expected, fields) in [
        (
            format!("properties=name&filter={loose}"),
            format!("{url}?filter={wanted}&properties=name"),
            serde_json::json!({"name": "String"}),
        ),
        (
            "properties=rank,name".to_owned(),
            format!("{url}?properties=rank%2Cname"),
            serde_json::json!({"name": "String", "rank": "Number"}),
        ),
    ] {
        let (status, head, body) = get(a, &format!("{tilejson}?{query}"), "").await;
        assert_eq!(status, "200", "{query}: {head}");
        let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(doc["tiles"][0].as_str(), Some(expected.as_str()), "{query}");
        assert_eq!(doc["vector_layers"][0]["fields"], fields, "{query}");
    }

    // TileJSON parameters share the tile route's validation and codes.
    let none = "/api/v1/datasets/tiles_none/tilejson.json";
    let bowtie = "%7B%22op%22%3A%22intersects%22%2C%22geometry%22%3A%7B%22type%22%3A%22Polygon%22%2C%22coordinates%22%3A%5B%5B%5B0%2C0%5D%2C%5B2%2C2%5D%2C%5B0%2C2%5D%2C%5B2%2C0%5D%2C%5B0%2C0%5D%5D%5D%7D%7D";
    for (target, code) in [
        (format!("{tilejson}?bbox=0,0,1,1"), req),
        (format!("{tilejson}?filter=nope"), bad),
        // Well-formed predicate whose bowtie polygon PostGIS rejects as invalid.
        (format!("{tilejson}?filter={bowtie}"), bad),
        (format!("{tilejson}?filter=%7B%7D&filter=%7B%7D"), req),
        (format!("{tilejson}?properties=name&properties=name"), req),
        (format!("{tilejson}?filter=%ZZ"), req),
        (format!("{tilejson}?filter"), req),
        (format!("{tilejson}?properties=name,"), req),
        (format!("{tilejson}?properties=secret"), req),
        (
            format!("{tilejson}?filter=%7B%22op%22%3A%22bogus%22%7D"),
            bad,
        ),
        (format!("{none}?filter=%7B"), bad),
        (format!("{none}?properties=name"), gone),
    ] {
        let (status, _, body) = get(a, &target, "").await;
        let expected = if code == gone { "404" } else { "400" };
        let found = String::from_utf8_lossy(&body).contains(code);
        assert!(status == expected && found, "{target}");
    }
    let private = format!("{private}?filter={wanted}&properties=name");
    assert_eq!(get(a, &private, "").await.0, "404");
    assert_eq!(get(a, &private, &viewer).await.0, "200");
    for url in [
        "ftp://atlas.test",
        "https://atlas.test/",
        "https://atlas.test?x",
        "https://a test",
    ] {
        let text = format!("public_url = '{url}'");
        assert!(Config::parse(&text, None, None).is_err(), "{url}");
    }
    assert!(Config::parse("public_url = 'http://localhost:8080/atlas'", None, None).is_ok());

    let doomed = "/api/v1/datasets/tiles_doomed/tiles/4/8/7.mvt";
    assert_eq!(get(a, doomed, "").await.0, "200");
    for statement in [
        "ALTER TABLE tiles_test.doomed DROP COLUMN name",
        "DROP TABLE tiles_test.doomed",
    ] {
        let mut tx = db.begin_catalog().await.unwrap();
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        let (status, _, body) = get(a, doomed, "").await;
        let found = String::from_utf8_lossy(&body).contains("schema_changed");
        assert!(status == "409" && found, "{statement}");
    }
}
