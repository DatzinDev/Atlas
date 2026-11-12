#![cfg(unix)]

use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Server(Child);

/// Every stderr line `start` has read so far, from all servers; lets a test wait for a late event.
static LOGGED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

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
fn weak_public_key_fails_before_listening() {
    let config = std::env::temp_dir().join(format!("atlas-weak-key-{}.toml", std::process::id()));
    std::fs::write(
        &config,
        "[auth]\npublic_key_file='tests/fixtures/jwt-test-1024-public.pem'\nissuer='atlas-test'\naudience='atlas'\n",
    )
    .unwrap();
    let address = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_atlas"))
            .args(["serve", "--config", config.to_str().unwrap()])
            .args(["--listen", &address])
            .env_remove("ATLAS_DATABASE_URL")
            .env_remove("ATLAS_LISTEN")
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut stderr = server.0.stderr.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    // EOF on stderr means the process exited; a listening server keeps it open.
    thread::spawn(move || {
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output);
        let _ = sender.send(output);
    });
    let output = receiver.recv_timeout(Duration::from_secs(10));
    let _ = std::fs::remove_file(&config);
    let output = output.expect("server with a 1024-bit key must exit, not listen");
    assert!(!server.0.wait().unwrap().success());
    assert_eq!(
        output.trim(),
        r#"{"event":"startup_failed","message":"invalid authentication configuration"}"#
    );
    assert!(TcpStream::connect(&address).is_err());
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

fn signed(claims: &serde_json::Value) -> String {
    let key =
        jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwt-test-private.pem"))
            .unwrap();
    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    jsonwebtoken::encode(&header, claims, &key).unwrap()
}

fn bearer(roles: &[&str]) -> String {
    let token = signed(
        &serde_json::json!({"sub": "alice", "iss": "atlas-test", "aud": "atlas",
        "exp": jsonwebtoken::get_current_timestamp() + 3600, "roles": roles}),
    );
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
    assert!(body["message"].is_string());
    let id = request_id(head);
    assert!(well_formed(id) && body["request_id"] == id, "{head}");
}

fn request_id(head: &str) -> &str {
    head.lines()
        .find_map(|line| line.strip_prefix("x-request-id:"))
        .map_or("", str::trim)
}

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

fn nonce(id: &str) -> &str {
    id.split_once('-').map_or(id, |(nonce, _)| nonce)
}

/// Spawns `atlas serve` on an ephemeral port; returns its address and stderr after `listening`.
fn start(command: &mut Command) -> (Server, String, thread::JoinHandle<Vec<String>>) {
    let mut server = Server(
        command
            .args(["--listen", "127.0.0.1:0"])
            .env_remove("ATLAS_LISTEN")
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stderr = server.0.stderr.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let logs = thread::spawn(move || {
        let mut lines = BufReader::new(stderr).lines();
        sender.send(lines.next().unwrap().unwrap()).unwrap();
        lines
            .map(Result::unwrap)
            .inspect(|line| LOGGED.lock().unwrap().push(line.clone()))
            .collect()
    });
    let listening = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    let address = listening
        .strip_prefix("{\"event\":\"listening\",\"address\":\"")
        .unwrap()
        .strip_suffix("\"}")
        .unwrap();
    (server, address.to_owned(), logs)
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
    let (server, address, logs) = start(
        Command::new(env!("CARGO_BIN_EXE_atlas"))
            .args(["serve", "--config"])
            .arg(&path)
            .env("ATLAS_DATABASE_URL", &dsn),
    );
    let _ = std::fs::remove_file(&path);
    // Every request is recorded to match it against its log line.
    let sent = std::cell::RefCell::new(Vec::new());
    let send = |raw: &str| {
        let response = call(&address, raw);
        sent.borrow_mut().push((raw.to_owned(), response.clone()));
        response
    };
    let request = |method: &str, target: &str, headers: &str| {
        send(&format!(
            "{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n{headers}\r\n"
        ))
    };
    let invalid = "Authorization: Bearer secret-token\r\n";
    let admin = bearer(&["atlas-admin"]);

    let body = r#"{"id":"main_http","title":"Main","source":{"schema":"main_http","table":"points"},"properties":["name"],"access":{"public":true,"public_download":true}}"#;
    let created = send(&format!(
        "POST /api/v1/admin/datasets HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{admin}\r\n{body}",
        body.len()
    ));
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
        ("GET", "/api/v1/nope?filter=secreto"),
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

    // The 401 hides the reason; only the `auth_failed` log carries it.
    let expired = signed(
        &serde_json::json!({"sub": "alice", "iss": "atlas-test", "aud": "atlas",
        "exp": jsonwebtoken::get_current_timestamp() - 3600, "roles": ["atlas-admin"]}),
    );
    let response = request(
        "GET",
        "/api/v1/datasets",
        &format!("Authorization: Bearer {expired}\r\n"),
    );
    assert_error(&response, 401, "invalid_token");
    let expired_id = request_id(&response.1).to_owned();

    let big = " ".repeat(256 * 1024 + 1);
    let oversized = send(&format!(
        "POST /api/v1/admin/datasets HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{admin}\r\n{big}",
        big.len()
    ));
    assert_error(&oversized, 413, "query_too_large");

    // 100 health ids: distinct, well-formed and sharing the process nonce.
    let ids: HashSet<_> = (0..100)
        .map(|_| request_id(&request("GET", "/api/v1/health", "").1).to_owned())
        .collect();
    let nonces: HashSet<_> = ids.iter().map(|id| nonce(id)).collect();
    assert!(ids.len() == 100 && ids.iter().all(|id| well_formed(id)) && nonces.len() == 1);

    // Another process without DSN: 503 ready, client id ignored, different nonce.
    let (_other, other, _) = start(
        Command::new(env!("CARGO_BIN_EXE_atlas"))
            .args(["serve", "--config", "tests/fixtures/jwt-test.toml"])
            .env_remove("ATLAS_DATABASE_URL"),
    );
    let ready = call(
        &other,
        "GET /api/v1/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nX-Request-Id: abc\r\n\r\n",
    );
    assert_error(&ready, 503, "database_unavailable");
    assert!(!nonces.contains(nonce(request_id(&ready.1))), "{}", ready.1);

    // One `event:request` line per request, matching the response; no secrets in stderr.
    drop(server);
    let stderr = logs.join().unwrap().join("\n");
    let events: Vec<serde_json::Value> = stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["event"] == "request")
        .collect();
    let sent = sent.into_inner();
    assert_eq!(events.len(), sent.len(), "one line per request");
    for (raw, (status, head, body)) in &sent {
        let id = request_id(head);
        let event = events.iter().find(|e| e["request_id"] == id).unwrap();
        let (method, target) = raw.split_once(' ').unwrap();
        assert_eq!(event["method"], method, "{event}");
        assert_eq!(
            event["path"],
            target.split([' ', '?']).next().unwrap(),
            "{event}"
        );
        assert_eq!(event["status"], *status, "{event}");
        assert!(event["duration_ms"].is_u64(), "{event}");
        let code = (*status >= 400)
            .then(|| serde_json::from_slice::<serde_json::Value>(body).unwrap()["code"].clone());
        assert_eq!(event.get("code"), code.as_ref(), "{event}");
    }

    let failure = stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| {
            event["event"] == "auth_failed" && event["request_id"] == expired_id.as_str()
        });
    assert_eq!(failure.unwrap()["reason"], "expired", "{stderr}");
    // Successful exports log no failure.
    assert!(!stderr.contains("export_failed"), "{stderr}");

    let token = admin.trim_end().rsplit(' ').next().unwrap();
    for secret in ["secreto", "secret-token", token, expired.as_str()] {
        assert!(!stderr.contains(secret), "{secret} leaked");
    }

    // An export cut by `export_bytes` logs one `export_failed` bound to its request id.
    std::fs::write(
        &path,
        "allowed_schemas = ['main_http']\n[limits]\nexport_bytes = 50\n",
    )
    .unwrap();
    let (server, address, logs) = start(
        Command::new(env!("CARGO_BIN_EXE_atlas"))
            .args(["serve", "--config"])
            .arg(&path)
            .env("ATLAS_DATABASE_URL", &dsn),
    );
    let _ = std::fs::remove_file(&path);
    let mut stream = TcpStream::connect(&address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET /api/v1/datasets/main_http/export HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    // The stream is aborted mid-body, so a reset after the partial body is expected.
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    drop(stream);
    // The export task logs after handing over the failing chunk: wait for it before killing.
    let id = request_id(&String::from_utf8_lossy(&response).to_lowercase()).to_owned();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && !LOGGED
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("export_failed") && line.contains(&id))
    {
        thread::sleep(Duration::from_millis(20));
    }
    drop(server);
    let stderr = logs.join().unwrap().join("\n");
    let events: Vec<serde_json::Value> = stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .collect();
    let exported: Vec<_> = events
        .iter()
        .filter(|e| e["event"] == "request" && e["path"] == "/api/v1/datasets/main_http/export")
        .collect();
    assert_eq!(exported.len(), 1, "{stderr}");
    assert_eq!(exported[0]["status"], 200, "{stderr}");
    let failed: Vec<_> = events
        .iter()
        .filter(|e| e["event"] == "export_failed")
        .collect();
    assert_eq!(failed.len(), 1, "one export_failed per stream: {stderr}");
    assert_eq!(failed[0]["code"], "query_too_large", "{stderr}");
    assert_eq!(
        failed[0]["request_id"], exported[0]["request_id"],
        "{stderr}"
    );
}

/// Trimmed value of the first header line starting with `name` (head is lowercased).
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .find_map(|line| line.strip_prefix(name))
        .map(str::trim)
}

/// Whether `tok` is among the comma-separated values of header `name`, across repeated lines.
fn has_token(head: &str, name: &str, tok: &str) -> bool {
    head.lines()
        .filter_map(|line| line.strip_prefix(name))
        .flat_map(|value| value.split(','))
        .any(|token| token.trim() == tok)
}

#[test]
fn cors_is_configurable_outermost_and_disabled_by_default() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let serve = |cors: &str| {
        let path = std::env::temp_dir().join(format!(
            "atlas-main-cors-{}-{}.toml",
            std::process::id(),
            cors.len()
        ));
        std::fs::write(
            &path,
            format!(
                "[auth]\npublic_key_file = '{}/tests/fixtures/jwt-test-public.pem'\nissuer = 'atlas-test'\naudience = 'atlas'\n{cors}",
                env!("CARGO_MANIFEST_DIR")
            ),
        )
        .unwrap();
        let started = start(
            Command::new(env!("CARGO_BIN_EXE_atlas"))
                .args(["serve", "--config"])
                .arg(&path)
                .env("ATLAS_DATABASE_URL", &dsn),
        );
        let _ = std::fs::remove_file(&path);
        started
    };
    let request = |address: &str, method: &str, target: &str, headers: &str| {
        call(
            address,
            &format!(
                "{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 0\r\n{headers}\r\n"
            ),
        )
    };
    let preflight = |origin: &str| {
        format!(
            "Origin: {origin}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization, content-type\r\n"
        )
    };
    let invalid = "Authorization: Bearer secret-token\r\n";
    let no_cors = |head: &str| {
        assert!(
            !head.lines().any(|line| line.starts_with("access-control-")),
            "{head}"
        );
    };

    // Disabled: no CORS headers anywhere and OPTIONS is a plain 405.
    let (server, address, _logs) = serve("");
    let response = request(
        &address,
        "OPTIONS",
        "/api/v1/datasets",
        &preflight("https://a.example"),
    );
    assert_error(&response, 405, "method_not_allowed");
    no_cors(&response.1);
    for headers in ["", invalid] {
        let (_, head, _) = request(
            &address,
            "GET",
            "/api/v1/datasets",
            &format!("Origin: https://a.example\r\n{headers}"),
        );
        no_cors(&head);
    }
    drop(server);

    // Listed origin: preflight needs no token; even a 401 carries CORS.
    let (server, address, _logs) = serve("[cors]\nallowed_origins = ['https://a.example']\n");
    let (status, head, _) = request(
        &address,
        "OPTIONS",
        "/api/v1/datasets/x/query",
        &preflight("https://a.example"),
    );
    assert!(
        (200..300).contains(&status)
            && header(&head, "access-control-allow-origin:") == Some("https://a.example")
            && has_token(&head, "access-control-allow-methods:", "post")
            && has_token(&head, "access-control-allow-headers:", "authorization")
            && header(&head, "access-control-max-age:") == Some("600")
            && has_token(&head, "vary:", "origin")
            && header(&head, "access-control-allow-credentials:").is_none(),
        "{head}"
    );
    let response = request(
        &address,
        "GET",
        "/api/v1/datasets",
        &format!("Origin: https://a.example\r\n{invalid}"),
    );
    assert_error(&response, 401, "invalid_token");
    assert!(
        header(&response.1, "access-control-allow-origin:") == Some("https://a.example")
            && has_token(
                &response.1,
                "access-control-expose-headers:",
                "x-request-id"
            ),
        "{}",
        response.1
    );

    // Unlisted origin: never allowed, on preflight or on a 401.
    for (method, target, headers) in [
        (
            "OPTIONS",
            "/api/v1/datasets/x/query",
            preflight("https://b.example"),
        ),
        (
            "GET",
            "/api/v1/datasets",
            format!("Origin: https://b.example\r\n{invalid}"),
        ),
    ] {
        let (_, head, _) = request(&address, method, target, &headers);
        assert!(
            header(&head, "access-control-allow-origin:").is_none(),
            "{head}"
        );
    }
    drop(server);

    // Wildcard.
    let (server, address, _logs) = serve("[cors]\nallowed_origins = ['*']\n");
    let (status, head, _) = request(
        &address,
        "OPTIONS",
        "/api/v1/datasets/x/query",
        &preflight("https://b.example"),
    );
    assert!(
        (200..300).contains(&status)
            && header(&head, "access-control-allow-origin:") == Some("*")
            && header(&head, "access-control-allow-credentials:").is_none(),
        "{head}"
    );
    drop(server);
}

#[tokio::test]
async fn ready_reports_capacity_exceeded_when_reserved_connections_are_busy() {
    use sqlx::Connection;
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let migrate = Command::new(env!("CARGO_BIN_EXE_atlas"))
        .arg("migrate")
        .env("ATLAS_DATABASE_URL", &dsn)
        .output()
        .unwrap();
    assert!(migrate.status.success());
    // pool_max - data_operations = 1 connection for readiness and catalog work.
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_atlas"))
            .args(["serve", "--config", "tests/fixtures/ready-capacity.toml"])
            .args(["--listen", "127.0.0.1:0"])
            .env("ATLAS_DATABASE_URL", &dsn)
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut lines = BufReader::new(server.0.stderr.take().unwrap()).lines();
    let listening = lines.next().unwrap().unwrap();
    // Keep draining stderr: a closed pipe makes the server's request log panic mid-response.
    thread::spawn(move || lines.for_each(drop));
    let address = listening
        .strip_prefix("{\"event\":\"listening\",\"address\":\"")
        .and_then(|rest| rest.strip_suffix("\"}"))
        .unwrap()
        .to_owned();
    let ready = || {
        let address = address.clone();
        thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            write!(
                stream,
                "GET /api/v1/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        })
    };
    let mut locker = sqlx::PgConnection::connect(&dsn).await.unwrap();
    let mut lock = locker.begin().await.unwrap();
    sqlx::query("LOCK atlas_catalog.datasets IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    // The first check blocks on the lock while holding the only reserved connection.
    let held = ready();
    let mut observer = sqlx::PgConnection::connect(&dsn).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_stat_activity WHERE application_name='atlas-rust' AND wait_event_type='Lock'")
        .fetch_one(&mut observer)
        .await
        .unwrap()
        == 0
    {
        assert!(Instant::now() < deadline, "readiness never blocked");
        thread::sleep(Duration::from_millis(10));
    }
    // The next check waits acquire_seconds for that connection, then reports capacity.
    let started = Instant::now();
    let refused = ready().join().unwrap();
    assert!(started.elapsed() >= Duration::from_millis(500));
    assert!(refused.starts_with("HTTP/1.1 503"), "{refused}");
    assert!(
        refused.contains(r#""code":"capacity_exceeded""#),
        "{refused}"
    );
    lock.rollback().await.unwrap();
    let held = held.join().unwrap();
    assert!(held.starts_with("HTTP/1.1 200"), "{held}");
}
