#![cfg(unix)]
use atlas::{config::Config, postgis::connection::Database};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Server(Child, String);
impl Server {
    fn start(config: &str, dsn: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_atlas"))
            .args(["serve", "--config", config, "--listen", "127.0.0.1:0"])
            .env("ATLAS_DATABASE_URL", dsn)
            .env_remove("ATLAS_LISTEN")
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (send, recv) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                let _ = send.send(line);
            }
        });
        let line = recv.recv_timeout(Duration::from_secs(10)).unwrap();
        let log: serde_json::Value = serde_json::from_str(&line).unwrap();
        Self(child, log["address"].as_str().unwrap().to_owned())
    }
    fn stop(&mut self) {
        Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn call(a: &str, method: &str, path: &str, auth: &str, body: &str) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(a).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s,"{method} /api/v1/{path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n{auth}\r\n{body}",body.len()).unwrap();
    let mut bytes = Vec::new();
    s.read_to_end(&mut bytes).unwrap();
    let split = bytes.windows(4).position(|b| b == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&bytes[..split]);
    let status = head[9..12].parse().unwrap();
    let mut body = bytes[split + 4..].to_vec();
    if head.to_lowercase().contains("transfer-encoding: chunked") {
        let mut out = Vec::new();
        let mut rest = body.as_slice();
        loop {
            let split = rest.windows(2).position(|b| b == b"\r\n").unwrap();
            let n =
                usize::from_str_radix(std::str::from_utf8(&rest[..split]).unwrap(), 16).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&rest[split + 2..split + 2 + n]);
            rest = &rest[split + 4 + n..];
        }
        body = out;
    }
    (status, body)
}
fn token(roles: &[&str]) -> String {
    let claims = serde_json::json!({"sub":"acceptance","iss":"atlas-test","aud":"atlas",
        "exp":jsonwebtoken::get_current_timestamp()+60,"roles":roles});
    let key =
        jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwt-test-private.pem"))
            .unwrap();
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        &key,
    )
    .unwrap();
    format!("Authorization: Bearer {token}\r\n")
}

#[tokio::test]
async fn experimental_mvp_survives_restart() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").unwrap();
    let c = Config::parse("", Some(dsn.clone()), None).unwrap();
    let db = Database::open(&c).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS acceptance_test CASCADE",
        "CREATE SCHEMA acceptance_test",
        "CREATE TABLE acceptance_test.data(id bigint PRIMARY KEY, n integer, geom geometry(Point,4326))",
        "INSERT INTO acceptance_test.data VALUES (1,1,ST_SetSRID(ST_Point(0,0),4326)),(2,2,ST_SetSRID(ST_Point(1,1),4326))",
        "CREATE VIEW acceptance_test.data_view AS SELECT * FROM acceptance_test.data",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    db.close().await;
    let file = std::env::temp_dir().join(format!("atlas-acceptance-{}.toml", std::process::id()));
    std::fs::write(&file,"allowed_schemas = ['acceptance_test']\n[auth]\npublic_key_file='tests/fixtures/jwt-test-public.pem'\nissuer='atlas-test'\naudience='atlas'\n").unwrap();
    let path = file.to_str().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_atlas"))
        .args(["migrate", "--config", path])
        .env("ATLAS_DATABASE_URL", &dsn)
        .output()
        .unwrap();
    assert!(output.status.success());
    let admin = token(&["atlas-admin"]);
    let reader = token(&["reader"]);
    let mut server = Server::start(path, &dsn);
    for (id, table, access) in [
        (
            "acceptance_public",
            "data",
            r#"{"public":true,"public_download":true}"#,
        ),
        (
            "acceptance_private",
            "data_view",
            r#"{"view_roles":["reader"],"query_roles":["reader"],"download_roles":["reader"]}"#,
        ),
    ] {
        let body = format!(
            r#"{{"id":"{id}","title":"MVP","source":{{"schema":"acceptance_test","table":"{table}","id_column":"id"}},"properties":["n"],"filter":{{"op":"gte","field":"n","value":2}},"access":{access}}}"#
        );
        // Clean only this test's previous publications, preserving source data.
        let (status, detail) = call(
            &server.1,
            "GET",
            &format!("admin/datasets/{id}"),
            &admin,
            "",
        );
        if status == 200 {
            let doc: serde_json::Value = serde_json::from_slice(&detail).unwrap();
            let auth = format!("{admin}If-Match: \"{}\"\r\n", doc["revision"]);
            assert_eq!(
                call(
                    &server.1,
                    "DELETE",
                    &format!("admin/datasets/{id}"),
                    &auth,
                    ""
                )
                .0,
                204
            );
        }
        assert_eq!(
            call(&server.1, "POST", "admin/datasets", &admin, &body).0,
            201
        );
    }
    server.stop();
    server = Server::start(path, &dsn);
    assert_eq!(call(&server.1, "GET", "ready", "", "").0, 200);
    for (id, auth) in [
        ("acceptance_public", ""),
        ("acceptance_private", reader.as_str()),
    ] {
        let base = format!("datasets/{id}");
        for route in [
            "",
            "/items",
            "/export",
            "/tilejson.json",
            "/tiles/0/0/0.mvt",
        ] {
            let (status, body) = call(&server.1, "GET", &format!("{base}{route}"), auth, "");
            assert_eq!(
                status,
                200,
                "{base}{route}: {}",
                String::from_utf8_lossy(&body)
            );
            if route == "/export" || route == "/items" {
                let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(doc["features"].as_array().unwrap().len(), 1);
                assert_eq!(doc["features"][0]["id"], 2);
            }
            if route.ends_with("mvt") {
                assert!(!body.is_empty());
            }
        }
    }
    assert_eq!(
        call(
            &server.1,
            "GET",
            "datasets/acceptance_private/items",
            "",
            ""
        )
        .0,
        404
    );
    assert_eq!(
        call(
            &server.1,
            "GET",
            "datasets/acceptance_private/export",
            &admin,
            ""
        )
        .0,
        404
    );
    server.stop();
    std::fs::remove_file(file).unwrap();
}
