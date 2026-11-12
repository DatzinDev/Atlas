#![cfg(unix)]
//! `docs/rust/openapi.yaml` checked against the real `atlas serve` binary and PostGIS.

use atlas::{config::Config, postgis::connection::Database};
use std::{
    collections::BTreeSet,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    time::Duration,
};

const SPEC: &str = include_str!("../../docs/rust/openapi.yaml");
const PROBED: [&str; 5] = ["get", "post", "patch", "delete", "put"];
const FIXTURE: &str = "openapi_contract";

/// Every `paths` entry with its declared methods, read line by line (no YAML crate).
fn operations() -> Vec<(String, Vec<&'static str>)> {
    let mut paths: Vec<(String, Vec<&str>)> = Vec::new();
    let mut inside = false;
    for line in SPEC.lines() {
        if !line.is_empty() && !line.starts_with(' ') {
            inside = line.trim_end() == "paths:";
            continue;
        }
        if !inside {
            continue;
        }
        let line = line.trim_end();
        if let Some(path) = line.strip_prefix("  /").and_then(|l| l.strip_suffix(':')) {
            paths.push((format!("/{path}"), Vec::new()));
        } else if let Some(method) = line
            .strip_prefix("    ")
            .and_then(|l| l.strip_suffix(':'))
            .filter(|m| PROBED.contains(m))
        {
            let (_, methods) = paths.last_mut().expect("method before any path");
            methods.push(method);
        }
    }
    paths
}

/// Lines of the `components.schemas.<name>` block.
fn schema(name: &str) -> Vec<&'static str> {
    let header = format!("    {name}:");
    let mut lines = SPEC.lines().skip_while(|l| l.trim_end() != header);
    assert!(lines.next().is_some(), "schema {name} missing");
    lines
        .take_while(|l| l.starts_with("      ") || l.trim().is_empty())
        .collect()
}

/// Top-level property names of a block-style schema.
fn properties(name: &str) -> BTreeSet<String> {
    let lines = schema(name);
    let start = lines
        .iter()
        .position(|l| l.trim_end() == "      properties:")
        .unwrap_or_else(|| panic!("{name} has no block properties"));
    lines[start + 1..]
        .iter()
        .take_while(|l| l.starts_with("        "))
        .filter_map(|l| {
            let rest = l.strip_prefix("        ")?;
            (!rest.starts_with(' ')).then(|| rest.split(':').next().unwrap().trim().to_owned())
        })
        .collect()
}

#[test]
fn spec_is_released_with_configurable_bounds() {
    assert!(SPEC.contains("\ninfo:\n") && SPEC.contains("\n  version: 0.1.0\n"));
    let lower = SPEC.to_lowercase();
    for banned in ["draft", "no implementado"] {
        assert!(!lower.contains(banned), "openapi.yaml mentions {banned:?}");
    }
    for (name, max) in [("Limit", "10.000"), ("Offset", "100.000")] {
        let header = format!("    {name}: ");
        let line = SPEC.lines().find(|l| l.starts_with(&header)).expect(name);
        let key = format!("max_{}", name.to_lowercase());
        assert!(line.contains(&key) && line.contains(max), "{line}");
        assert!(!line.contains("maximum"), "{line}");
    }
    for line in schema("Query") {
        let bounded = matches!(
            line.trim_start().split(':').next(),
            Some("limit" | "offset")
        );
        assert!(!bounded || !line.contains("maximum"), "{line}");
    }
    let config = "config: {$ref: '#/components/schemas/PublicationCreate'}";
    assert!(schema("Publication").iter().any(|l| l.contains(config)));
}

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
        // ponytail: no startup timeout; a server that exits closes stderr and fails here.
        let stderr = BufReader::new(child.stderr.take().unwrap());
        let mut lines = stderr.lines().map_while(Result::ok);
        let address = lines.find_map(|line| {
            let log: serde_json::Value = serde_json::from_str(&line).ok()?;
            (log["event"] == "listening").then(|| log["address"].as_str().unwrap().to_owned())
        });
        std::thread::spawn(move || lines.for_each(drop));
        Self(child, address.expect("server exited before `listening`"))
    }

    /// Returns status, lower-cased head and body. Error bodies are never chunked.
    fn call(&self, verb: &str, target: &str, headers: &str, body: &str) -> (u16, String, Vec<u8>) {
        let mut s = TcpStream::connect(&self.1).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(
            s,
            "{verb} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut bytes = Vec::new();
        s.read_to_end(&mut bytes).unwrap();
        let split = bytes.windows(4).position(|b| b == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&bytes[..split]).to_lowercase();
        let status = head[9..12].parse().unwrap();
        let body = bytes[split + 4..].to_vec();
        (status, head, body)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn admin() -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, get_current_timestamp};
    let claims = serde_json::json!({"sub":"openapi","iss":"atlas-test","aud":"atlas",
        "exp":get_current_timestamp()+60,"roles":["atlas-admin"]});
    let key = EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwt-test-private.pem")).unwrap();
    let token = encode(&Header::new(Algorithm::RS256), &claims, &key).unwrap();
    format!("Authorization: Bearer {token}\r\n")
}

fn code(body: &[u8]) -> String {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    v["code"].as_str().unwrap_or_default().to_owned()
}

fn concrete(path: &str) -> String {
    let target = format!("/api/v1{path}")
        .replace("{id}", FIXTURE)
        .replace("{z}", "0")
        .replace("{x}", "0")
        .replace("{y}", "0");
    assert!(!target.contains('{'), "unknown template in {path}");
    target
}

#[tokio::test]
async fn openapi_matches_the_running_server() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let c = Config::parse("", Some(dsn.clone()), None).unwrap();
    let db = Database::open(&c).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS openapi_test CASCADE",
        "CREATE SCHEMA openapi_test",
        "CREATE TABLE openapi_test.data(id bigint PRIMARY KEY, n integer, geom geometry(Point,4326))",
        "INSERT INTO openapi_test.data VALUES (1,1,ST_SetSRID(ST_Point(0,0),4326))",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    db.close().await;
    let file = std::env::temp_dir().join(format!("atlas-openapi-{}.toml", std::process::id()));
    std::fs::write(&file, "allowed_schemas = ['openapi_test']\n[auth]\npublic_key_file='tests/fixtures/jwt-test-public.pem'\nissuer='atlas-test'\naudience='atlas'\n").unwrap();
    let path = file.to_str().unwrap();
    let migrated = Command::new(env!("CARGO_BIN_EXE_atlas"))
        .args(["migrate", "--config", path])
        .env("ATLAS_DATABASE_URL", &dsn)
        .output()
        .unwrap();
    assert!(migrated.status.success());
    let server = Server::start(path, &dsn);
    let _ = std::fs::remove_file(&file);
    let admin = admin();
    let json = format!("{admin}Content-Type: application/json\r\n");

    // Fixture publication, recreated from scratch; public so dataset routes resolve.
    let detail = format!("/api/v1/admin/datasets/{FIXTURE}");
    let (status, _, body) = server.call("GET", &detail, &admin, "");
    if status == 200 {
        let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let auth = format!("{admin}If-Match: \"{}\"\r\n", doc["revision"]);
        assert_eq!(server.call("DELETE", &detail, &auth, "").0, 204);
    }
    let create = format!(
        r#"{{"id":"{FIXTURE}","title":"OpenAPI","source":{{"schema":"openapi_test","table":"data","id_column":"id"}},"properties":["n"],"access":{{"public":true,"public_download":true}}}}"#
    );
    let (status, head, body) = server.call("POST", "/api/v1/admin/datasets", &json, &create);
    assert_eq!(status, 201, "{head}{}", String::from_utf8_lossy(&body));

    // H5: every key the admin detail returns in `config` is declared by PublicationCreate.
    let (status, _, body) = server.call("GET", &detail, &admin, "");
    assert_eq!(status, 200);
    let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let revision = doc["revision"].as_i64().unwrap();
    let declared = properties("PublicationCreate");
    let config = doc["config"].as_object().expect("config object");
    assert!(config.contains_key("id"), "{config:?}");
    for key in config.keys() {
        assert!(declared.contains(key), "config.{key} not in {declared:?}");
    }

    let mut probed = 0;
    let stale = format!("{admin}If-Match: \"0\"\r\n");
    let patch = format!("{stale}Content-Type: application/merge-patch+json\r\n");
    for (path, methods) in &operations() {
        let target = concrete(path);
        // Declared operations must be routed. Mutations use a stale revision so the
        // fixture survives; POST bodies are minimal and never create anything.
        for method in methods {
            let (headers, body) = match *method {
                // Empty query on the dataset; invalid (400) creation on admin.
                "post" => (json.clone(), "{}"),
                "patch" => (patch.clone(), "{}"),
                "delete" => (stale.clone(), ""),
                _ => (admin.clone(), ""),
            };
            let verb = method.to_uppercase();
            let (status, head, body) = server.call(&verb, &target, &headers, body);
            let error = code(&body);
            let unrouted = matches!(error.as_str(), "not_found" | "method_not_allowed");
            let routed = !matches!(status, 404 | 405) && !unrouted;
            assert!(routed, "{verb} {target} -> {status} {error}\n{head}");
            probed += 1;
        }
        // Undeclared methods are 405 method_not_allowed, never a 404 fallback.
        for method in PROBED.iter().filter(|m| !methods.contains(*m)) {
            let verb = method.to_uppercase();
            let (status, head, body) = server.call(&verb, &target, &admin, "");
            assert_eq!(status, 405, "{verb} {target}\n{head}");
            assert_eq!(code(&body), "method_not_allowed", "{verb} {target}");
        }
    }
    assert_eq!(probed, SPEC.matches("operationId:").count());

    // Explicit acceptance cases.
    for (verb, target) in [("PUT", detail.as_str()), ("POST", "/api/v1/datasets")] {
        let (status, _, body) = server.call(verb, target, &admin, "");
        assert_eq!(status, 405, "{verb} {target}");
        assert_eq!(code(&body), "method_not_allowed", "{verb} {target}");
    }

    // The probes left the fixture untouched.
    let (status, _, body) = server.call("GET", &detail, &admin, "");
    assert_eq!(status, 200);
    let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(doc["revision"].as_i64(), Some(revision));
    let auth = format!("{admin}If-Match: \"{revision}\"\r\n");
    assert_eq!(server.call("DELETE", &detail, &auth, "").0, 204);
}
