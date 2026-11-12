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
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            let Ok(line) = recv.recv_timeout(wait) else {
                let _ = child.kill();
                let _ = child.wait();
                panic!("atlas serve did not report listening within 10s");
            };
            let log: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
            if log["event"] == "listening" {
                return Self(child, log["address"].as_str().unwrap().to_owned());
            }
        }
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
    let admin_json = format!("{admin}Content-Type: application/json\r\n");
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
            call(&server.1, "POST", "admin/datasets", &admin_json, &body).0,
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

fn read(rel: &str) -> Option<String> {
    std::fs::read_to_string(format!("{}/{rel}", env!("CARGO_MANIFEST_DIR"))).ok()
}
fn test_source(file: &str) -> Option<String> {
    read(&format!("tests/{file}.rs"))
}
fn has_token(text: &str, token: &str) -> bool {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w == token)
}
/// Matrix rows whose first cell names `id`.
fn rows<'m>(matrix: &'m str, id: &str) -> Vec<&'m str> {
    matrix
        .lines()
        .filter(|l| {
            l.trim_start()
                .strip_prefix('|')
                .is_some_and(|rest| has_token(rest.split('|').next().unwrap_or(""), id))
        })
        .collect()
}
/// `file::function` entries written as code spans in `rows`.
fn entries<'m>(rows: &[&'m str]) -> Vec<(&'m str, &'m str)> {
    let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    rows.iter()
        .copied()
        .flat_map(|r| r.split('`').skip(1).step_by(2))
        .filter_map(|span| span.split_once("::"))
        .filter(|&(f, t)| ident(f) && ident(t))
        .collect()
}
fn trace_problems(matrix: &str, source: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut problems = Vec::new();
    for n in 1..=17 {
        let id = format!("R{n:02}");
        let rows = rows(matrix, &id);
        let entries = entries(&rows);
        if rows.is_empty() {
            problems.push(format!("{id}: missing matrix row"));
        } else if n == 17 && !rows.iter().any(|r| r.contains("CI")) {
            problems.push(format!("{id}: must cite the Python CI job as exception"));
        } else if n < 17 && entries.is_empty() {
            problems.push(format!("{id}: no file::function entry"));
        }
        for (file, test) in entries {
            let Some(code) = source(file) else {
                problems.push(format!("{id}: {file}::{test}: tests/{file}.rs not found"));
                continue;
            };
            let lines: Vec<&str> = code.lines().collect();
            let Some(at) = lines
                .iter()
                .position(|l| l.contains(&format!("fn {test}(")))
            else {
                problems.push(format!("{id}: {file}::{test}: function not found"));
                continue;
            };
            // Attributes and comments directly above the fn, closest first.
            let header: Vec<&str> = lines[..at]
                .iter()
                .rev()
                .map(|l| l.trim())
                .take_while(|l| l.starts_with("#[") || l.starts_with("//"))
                .collect();
            let attr = header
                .iter()
                .position(|l| *l == "#[test]" || l.starts_with("#[tokio::test"));
            let problem = match attr {
                None => "not a test".to_owned(),
                Some(a)
                    if !header[a + 1..]
                        .iter()
                        .any(|l| l.starts_with("//") && has_token(l, &id)) =>
                {
                    format!("no // {id} above its attribute")
                }
                Some(_) => continue,
            };
            problems.push(format!("{id}: {file}::{test}: {problem}"));
        }
    }
    problems
}
fn acceptance_matrix() -> String {
    read("../docs/rust/acceptance.md").unwrap()
}

#[test]
fn requirements_are_traced() {
    let problems = trace_problems(&acceptance_matrix(), &test_source);
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn requirements_trace_fails_when_a_row_is_removed() {
    let matrix = acceptance_matrix();
    for n in 1..=17 {
        let id = format!("R{n:02}");
        let removed = rows(&matrix, &id);
        assert!(!removed.is_empty(), "{id}");
        let without: String = matrix
            .lines()
            .filter(|l| !removed.contains(l))
            .map(|l| format!("{l}\n"))
            .collect();
        let problems = trace_problems(&without, &test_source);
        assert!(
            problems.contains(&format!("{id}: missing matrix row")),
            "{id}: {problems:#?}"
        );
    }
}

#[test]
fn requirements_trace_fails_when_a_cited_test_is_renamed_or_deleted() {
    let matrix = acceptance_matrix();
    let cited: Vec<_> = (1..=17)
        .flat_map(|n| entries(&rows(&matrix, &format!("R{n:02}"))))
        .collect();
    assert!(!cited.is_empty());
    for (file, test) in cited {
        let check = |source: &dyn Fn(&str) -> Option<String>, problem: String| {
            let entry = format!("{file}::{test}: {problem}");
            let problems = trace_problems(&matrix, source);
            assert!(
                problems.iter().any(|p| p.ends_with(&entry)),
                "{entry}: {problems:#?}"
            );
        };
        let renamed = |f: &str| {
            let code = test_source(f)?;
            Some(if f == file {
                code.replace(&format!("fn {test}("), &format!("fn {test}_renamed("))
            } else {
                code
            })
        };
        check(&renamed, "function not found".to_owned());
        check(
            &|f: &str| test_source(f).filter(|_| f != file),
            format!("tests/{file}.rs not found"),
        );
    }
}

#[test]
fn requirements_trace_checks_comments_attributes_and_entries() {
    let mut matrix = String::from("| Requisito | Evidencia |\n|---|---|\n");
    let mut good = String::new();
    for n in 1..=16 {
        matrix.push_str(&format!("| R{n:02} x | `t::f{n}` |\n"));
        good.push_str(&format!(
            "// R{n:02}\n#[tokio::test]\nasync fn f{n}() {{}}\n"
        ));
    }
    matrix.push_str("| R17 convivencia Python | job Python de CI |\n");
    let missing = "R01: t::f1: no // R01 above its attribute";
    // Each edit is applied to both the matrix and the source; "" expects no problem.
    for (from, to, expected) in [
        ("", "", ""),
        // Comment missing, naming another R-ID, or below the attribute.
        ("// R01\n", "", missing),
        ("// R01\n", "// R010 R11\n", missing),
        (
            "// R01\n#[tokio::test]\n",
            "#[tokio::test]\n// R01\n",
            missing,
        ),
        // Plain #[test] with a multi-ID comment is accepted.
        (
            "// R01\n#[tokio::test]\nasync fn f1()",
            "// R01, R04\n#[test]\nfn f1()",
            "",
        ),
        // A helper is not a test, and a prefix match is not the cited function.
        (
            "#[tokio::test]\nasync fn f1()",
            "fn f1()",
            "R01: t::f1: not a test",
        ),
        ("fn f1(", "fn f1_other(", "R01: t::f1: function not found"),
        // Regression: module-only evidence (the old matrix format) is rejected.
        (
            "`t::f2`",
            "`api_admin`, `catalog`",
            "R02: no file::function entry",
        ),
        // R17 needs no test but must stay marked as the CI exception.
        (
            "job Python de CI",
            "pendiente",
            "R17: must cite the Python CI job as exception",
        ),
    ] {
        let code = good.replacen(from, to, 1);
        let source = |f: &str| (f == "t").then(|| code.clone());
        let problems = trace_problems(&matrix.replacen(from, to, 1), &source);
        assert_eq!(problems.join("\n"), expected, "{from:?} -> {to:?}");
    }
}
