#![cfg(unix)]

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn configuration_errors_do_not_echo_input() {
    for args in [
        vec![],
        vec!["serve", "--unknown", "secret-token"],
        vec!["serve", "--listen", "postgres://user:password@host/db"],
        vec!["serve", "--config", "/missing/secret-token.toml"],
        vec!["serve", "--config", "tests/fixtures/jwt-test-invalid.toml"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_atlas"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("startup_failed"));
        assert!(!error.contains("secret-token"));
        assert!(!error.contains("password"));
    }
}

#[test]
fn http_startup_and_shutdown() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    assert!(
        Command::new(env!("CARGO_BIN_EXE_atlas"))
            .arg("migrate")
            .env("ATLAS_DATABASE_URL", &dsn)
            .output()
            .unwrap()
            .status
            .success()
    );
    for (signal, database, auth) in [
        ("-TERM", None, false),
        ("-INT", None, false),
        ("-TERM", Some(&dsn), true),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_atlas"));
        command.arg("serve");
        command
            .env_remove("ATLAS_DATABASE_URL")
            .env_remove("ATLAS_LISTEN");
        if let Some(dsn) = database {
            command.env("ATLAS_DATABASE_URL", dsn);
        }
        if auth {
            command.args(["--config", "tests/fixtures/jwt-test.toml"]);
        }
        let mut server = Server(
            command
                .args(["--listen", "127.0.0.1:0"])
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let stderr = server.0.stderr.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let logs = thread::spawn(move || {
            let mut lines = BufReader::new(stderr).lines();
            let first = lines.next().unwrap().unwrap();
            sender.send(first).unwrap();
            lines.map(Result::unwrap).collect::<Vec<_>>().join("\n")
        });
        let listening = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        let address = listening
            .strip_prefix("{\"event\":\"listening\",\"address\":\"")
            .unwrap()
            .strip_suffix("\"}")
            .unwrap();
        for (path, status, authorization) in [
            ("health", "200", ""),
            ("ready", if database.is_some() { "200" } else { "503" }, ""),
            (
                "datasets",
                if database.is_some() { "200" } else { "404" },
                "",
            ),
            ("health", "200", "Authorization: Bearer secret-token\r\n"),
            (
                "ready",
                if database.is_some() { "200" } else { "503" },
                "Authorization: Bearer secret-token\r\n",
            ),
        ] {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            write!(
                stream,
                "GET /api/v1/{path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{authorization}\r\n"
            )
            .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(
                response.starts_with(&format!("HTTP/1.1 {status}")),
                "{response}"
            );
            let head = response.to_lowercase();
            assert!(
                head.contains("x-content-type-options: nosniff"),
                "{response}"
            );
            if path == "ready" && database.is_none() {
                assert!(
                    response.contains("\"code\":\"database_unavailable\"")
                        && response.contains("request_id")
                        && head.contains("cache-control: no-store")
                        && head.contains("content-type: application/json"),
                    "{response}"
                );
            }
            if path == "datasets" && database.is_none() {
                assert!(response.contains("\"code\":\"not_found\""), "{response}");
            }
        }
        let occupied = Command::new(env!("CARGO_BIN_EXE_atlas"))
            .args(["serve", "--listen", address])
            .output()
            .unwrap();
        assert!(!occupied.status.success());
        assert!(
            String::from_utf8(occupied.stderr)
                .unwrap()
                .contains("cannot bind listener")
        );
        assert!(
            Command::new("kill")
                .args([signal, &server.0.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = server.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "shutdown timed out");
            thread::sleep(Duration::from_millis(10));
        }
        let logs = logs.join().unwrap();
        assert!(logs.contains("shutdown_complete") && !logs.contains("secret-token"));
    }
}

fn call(address: &str, request: &str) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&response[..split]).to_lowercase();
    let status = head[9..12].parse().unwrap();
    (status, head, response[split + 4..].to_vec())
}

fn bearer(roles: &[&str]) -> String {
    let claims = serde_json::json!({"sub": "alice", "iss": "atlas-test", "aud": "atlas",
        "exp": jsonwebtoken::get_current_timestamp() + 3600, "roles": roles});
    let key =
        jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwt-test-private.pem"))
            .unwrap();
    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    let token = jsonwebtoken::encode(&header, &claims, &key).unwrap();
    format!("Authorization: Bearer {token}\r\n")
}

fn assert_error(response: &(u16, String, Vec<u8>), status: u16, code: &str) {
    let (actual, head, body) = response;
    assert_eq!(*actual, status, "{head}");
    assert!(
        head.contains("content-type: application/json")
            && head.contains("cache-control: no-store")
            && head.contains("x-content-type-options: nosniff"),
        "{head}"
    );
    let body: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(body["code"], code, "{head}");
    assert!(body["message"].is_string() && body["request_id"].is_string());
}

#[test]
fn http_fallback_public_probes_and_nosniff() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let config = atlas::config::Config::parse("", Some(dsn.clone()), None).unwrap();
        atlas::migrations::migrate(&config).await.unwrap();
        let db = atlas::postgis::connection::Database::open(&config)
            .await
            .unwrap();
        let mut tx = db.begin_catalog().await.unwrap();
        for statement in [
            "DROP SCHEMA IF EXISTS main_http CASCADE",
            "CREATE SCHEMA main_http",
            "CREATE TABLE main_http.points(id integer PRIMARY KEY, name text, geom geometry(Point,4326))",
            "INSERT INTO main_http.points VALUES (1,'a',ST_SetSRID(ST_Point(10,10),4326))",
            "DELETE FROM atlas_catalog.datasets WHERE id = 'main_http'",
        ] {
            sqlx::query(statement).execute(&mut *tx).await.unwrap();
        }
        tx.commit().await.unwrap();
        db.close().await;
    });
    let path = std::env::temp_dir().join(format!("atlas-main-http-{}.toml", std::process::id()));
    std::fs::write(
        &path,
        format!(
            "allowed_schemas = ['main_http']\n[auth]\npublic_key_file = '{}/tests/fixtures/jwt-test-public.pem'\nissuer = 'atlas-test'\naudience = 'atlas'\n",
            env!("CARGO_MANIFEST_DIR")
        ),
    )
    .unwrap();
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_atlas"))
            .args(["serve", "--listen", "127.0.0.1:0", "--config"])
            .arg(&path)
            .env("ATLAS_DATABASE_URL", &dsn)
            .env_remove("ATLAS_LISTEN")
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stderr = server.0.stderr.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut lines = BufReader::new(stderr).lines();
        sender.send(lines.next().unwrap().unwrap()).unwrap();
        lines.for_each(drop);
    });
    let listening = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    let _ = std::fs::remove_file(&path);
    let address = listening
        .strip_prefix("{\"event\":\"listening\",\"address\":\"")
        .unwrap()
        .strip_suffix("\"}")
        .unwrap();
    let request = |method: &str, target: &str, headers: &str| {
        call(
            address,
            &format!(
                "{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n{headers}\r\n"
            ),
        )
    };
    let invalid = "Authorization: Bearer secret-token\r\n";

    let body = r#"{"id":"main_http","title":"Main","source":{"schema":"main_http","table":"points"},"properties":["name"],"access":{"public":true,"public_download":true}}"#;
    let created = call(
        address,
        &format!(
            "POST /api/v1/admin/datasets HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}\r\n{body}",
            body.len(),
            bearer(&["atlas-admin"])
        ),
    );
    assert_eq!(created.0, 201, "{}", created.1);
    assert!(created.1.contains("x-content-type-options: nosniff"));

    // Successful tile and streamed export carry nosniff.
    let tile = "/api/v1/datasets/main_http/tiles/4/8/7.mvt";
    for target in [tile, "/api/v1/datasets/main_http/export"] {
        let (status, head, _) = request("GET", target, "");
        assert_eq!(status, 200, "{target}: {head}");
        assert!(head.contains("x-content-type-options: nosniff"), "{head}");
    }

    // Unknown routes, any method, use the error model.
    for (method, target) in [
        ("GET", "/api/v1/nope"),
        ("GET", "/"),
        ("POST", "/api/v1/nope"),
        ("GET", "/api/v1/health/"),
        ("GET", "/api/v1/datasets/main_http/nope"),
    ] {
        assert_error(&request(method, target, ""), 404, "not_found");
    }

    // Known routes with another method keep Allow and use the error model.
    for (method, target) in [
        ("PUT", "/api/v1/datasets"),
        ("DELETE", "/api/v1/health"),
        ("POST", "/api/v1/ready"),
        ("POST", tile),
    ] {
        let response = request(method, target, "");
        assert_error(&response, 405, "method_not_allowed");
        let allow = response
            .1
            .lines()
            .find_map(|line| line.strip_prefix("allow:"))
            .unwrap_or_else(|| panic!("{method} {target}: missing allow"));
        assert!(allow.split(',').any(|m| m.trim() == "get"), "{allow}");
    }

    // dataset_not_found stays reserved to datasets.
    assert_error(
        &request("GET", "/api/v1/datasets/main_missing", ""),
        404,
        "dataset_not_found",
    );

    // Probes ignore any Authorization header.
    for headers in [
        invalid.to_owned(),
        "Authorization: Basic secret-token\r\n".to_owned(),
        "Authorization: Bearer\r\n".to_owned(),
        format!("{invalid}{invalid}"),
        bearer(&[]),
    ] {
        for target in ["/api/v1/health", "/api/v1/ready"] {
            let (status, head, body) = request("GET", target, &headers);
            assert_eq!(status, 200, "{target}: {head}");
            assert!(head.contains("x-content-type-options: nosniff"), "{head}");
            assert!(
                !head.contains("secret-token") && !body.windows(12).any(|w| w == b"secret-token")
            );
        }
    }

    // Data routes still authenticate.
    for target in [
        tile,
        "/api/v1/datasets",
        "/api/v1/datasets/main_http/export",
    ] {
        let response = request("GET", target, invalid);
        assert_error(&response, 401, "invalid_token");
        assert!(response.1.contains("www-authenticate: bearer"));
    }
}
