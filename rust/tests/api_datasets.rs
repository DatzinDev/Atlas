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
        "CREATE TABLE discovery_test.places(id integer PRIMARY KEY, name text, geom geometry(Point,4326)) WITH (autovacuum_enabled=false)",
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

// R03
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
        ("/disc_a_public?unknown=1", "", "400"),
        ("/disc_a_public?limit=1&limit=2", "", "400"),
        ("/disc_none?unknown=1", "", "400"),
        ("/disc_none?limit=1&limit=2", viewer.as_str(), "400"),
        ("/disc_a_public?", "", "200"),
        ("", "Authorization: Bearer forged\r\n", "401"),
    ] {
        let (status, body) = get(a, &format!("{LIST}{target}"), headers).await;
        assert_eq!(status, expected, "{target}: {body}");
        if expected == "400" {
            assert!(body["code"].is_string() && body["request_id"].is_string());
        }
        assert!(!body.to_string().contains("discovery_test"));
    }

    let public = br#"{"access":{"public":true}}"#;
    service.update("disc_b_view", public, 1).await.unwrap();
    assert_eq!(page(a, "", "").await.0, "disc_a_public,disc_b_view");

    // An empty typed table publishes `"extent": null`, present in detail and list.
    let (_, detail) = get(a, &format!("{LIST}/disc_a_public"), "").await;
    assert_eq!(
        detail.get("extent"),
        Some(&serde_json::Value::Null),
        "{detail}"
    );
    let (_, list) = get(a, LIST, "").await;
    assert_eq!(list["datasets"][0]["extent"], serde_json::Value::Null);
    let tilejson = service
        .tilejson(&service.get("disc_a_public").await.unwrap(), None, None)
        .await
        .unwrap();

    // New rows do not move the published snapshot; a PATCH refreshes it.
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").unwrap()),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["discovery_test".into()];
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("INSERT INTO discovery_test.places VALUES (1,'a',ST_SetSRID(ST_Point(-100,19),4326)), (2,'b',ST_SetSRID(ST_Point(-98,21),4326))")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    db.close().await;
    let (_, detail) = get(a, &format!("{LIST}/disc_a_public"), "").await;
    assert_eq!(detail["extent"], serde_json::Value::Null);
    service
        .update("disc_a_public", br#"{"title":"Located"}"#, 1)
        .await
        .unwrap();
    let expected = serde_json::json!([-100.0, 19.0, -98.0, 21.0]);
    let (status, detail) = get(a, &format!("{LIST}/disc_a_public"), "").await;
    assert_eq!(status, "200");
    assert_eq!(detail["extent"], expected, "{detail}");
    let (_, list) = get(a, LIST, "").await;
    let located = list["datasets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "disc_a_public")
        .unwrap();
    assert_eq!(located["extent"], expected);
    let (_, detail) = get(a, &format!("{LIST}/disc_c_query"), &viewer).await;
    assert_eq!(detail["extent"], serde_json::Value::Null);

    // TileJSON is unchanged: no extent or bounds leak into it.
    let refreshed = service
        .tilejson(&service.get("disc_a_public").await.unwrap(), None, None)
        .await
        .unwrap();
    let mut tilejson = tilejson;
    tilejson["name"] = serde_json::json!("Located");
    assert_eq!(tilejson, refreshed);
    assert!(refreshed.get("bounds").is_none() && refreshed.get("extent").is_none());

    discovery_scales_past_one_batch(a, &config).await;
}

/// Ids of `disc_s_NNN` rows that a principal with `roles` may see (`None`: anonymous).
fn expected(roles: Option<&[&str]>) -> Vec<String> {
    let has = |role: &str| roles.is_some_and(|r| r.contains(&role));
    (0..300)
        .filter(|g| {
            // Non-array `view_roles`, `enabled` that is not a boolean, an invalid source.
            ![7, 13, 19, 30, 42, 48].contains(g)
                && match g % 6 {
                    0 => true,
                    1 => has("viewer"),
                    2 => has("other"),
                    _ => false,
                }
        })
        .map(|g| format!("disc_s_{g:03}"))
        .collect()
}

/// 300 rows (more than one 256-row batch) mixing every visibility case.
async fn discovery_scales_past_one_batch(a: SocketAddr, config: &Config) {
    let db = Database::open(config).await.unwrap();
    let id = "format('disc_s_%s', lpad(g::text, 3, '0'))";
    let access = r#"CASE g % 6 WHEN 0 THEN '{"public":true}' WHEN 1 THEN '{"view_roles":["viewer"]}' WHEN 2 THEN '{"view_roles":["other"]}' WHEN 3 THEN '{"public":true}' WHEN 4 THEN '{"view_roles":["viewer"]}' ELSE '{"view_roles":[]}' END::jsonb"#;
    let row = |g: u32, set: &str| {
        format!("UPDATE atlas_catalog.datasets SET config = {set} WHERE id = 'disc_s_{g:03}'")
    };
    let statements = [
        format!("INSERT INTO atlas_catalog.datasets(id, config, metadata) SELECT {id}, jsonb_set(jsonb_set(jsonb_set(d.config, '{{id}}', to_jsonb({id})), '{{access}}', {access}), '{{enabled}}', to_jsonb(g % 6 NOT IN (3, 4))), d.metadata FROM atlas_catalog.datasets d, generate_series(0, 299) g WHERE d.id = 'disc_a_public'"),
        "DELETE FROM atlas_catalog.datasets WHERE id LIKE 'disc\\_%' AND id NOT LIKE 'disc\\_s\\_%'".into(),
        // `?|` matches a scalar string or an object key: SQL keeps them, decode drops them.
        row(7, r#"jsonb_set(config, '{access,view_roles}', '"viewer"')"#),
        row(13, r#"jsonb_set(config, '{access,view_roles}', '{"viewer":1}')"#),
        row(19, "jsonb_set(config, '{access,view_roles}', '5')"),
        row(48, r#"jsonb_set(config, '{access,view_roles}', '"viewer"')"#),
        // A missing `enabled` means enabled; a string "false" is not a boolean.
        row(24, "config - 'enabled'"),
        row(30, r#"jsonb_set(config, '{enabled}', '"false"')"#),
        row(42, r#"jsonb_set(config, '{source,schema}', '"elsewhere"')"#),
    ];
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in &statements {
        sqlx::query(sqlx::AssertSqlSafe(statement.clone()))
            .execute(&mut *tx)
            .await
            .unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
    tx.commit().await.unwrap();

    let (viewer, both, cased, none) = (
        bearer(&["viewer"]),
        bearer(&["other", "viewer"]),
        bearer(&["Viewer", "VIEWER"]),
        bearer(&[]),
    );
    for (headers, roles) in [
        ("", None),
        (viewer.as_str(), Some(&["viewer"][..])),
        (both.as_str(), Some(&["other", "viewer"][..])),
        (cased.as_str(), Some(&[][..])),
        (none.as_str(), Some(&[][..])),
    ] {
        let ids = expected(roles);
        let n = ids.len();
        assert_eq!(page(a, "limit=1000", headers).await, (ids.join(","), false));
        for offset in (0..=n + 50).step_by(50) {
            let slice = &ids[offset.min(n)..(offset + 50).min(n)];
            assert_eq!(
                page(a, &format!("limit=50&offset={offset}"), headers).await,
                (slice.join(","), offset + 50 < n),
                "{roles:?} offset {offset}"
            );
        }
        assert_eq!(
            page(a, &format!("limit={}", n - 1), headers).await,
            (ids[..n - 1].join(","), true)
        );
        assert_eq!(
            page(a, &format!("limit=1&offset={}", n - 1), headers).await,
            (ids[n - 1].clone(), false)
        );
    }
    // The rows with a non-array `view_roles` are still absent from detail.
    for g in [7, 13, 48] {
        let (status, _) = get(a, &format!("{LIST}/disc_s_{g:03}"), &viewer).await;
        assert_eq!(status, "404", "{g}");
    }

    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("DELETE FROM atlas_catalog.datasets WHERE id LIKE 'disc\\_%'")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    db.close().await;
}
