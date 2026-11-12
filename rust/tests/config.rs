use atlas::config::Config;

#[test]
fn defaults_and_overrides_are_validated() {
    let config = Config::parse("", None, None).unwrap();
    assert_eq!(config.limits.max_limit, 10000);
    assert_eq!(config.limits.pool_max, 10);
    assert_eq!(config.limits.export_bytes, 268435456);
    assert_eq!(config.limits.data_operations, 8);
    assert_eq!(config.limits.export_operations, 2);
    assert_eq!(config.limits.page_bytes, 33554432);
    for budget in [1, 1073741824] {
        let text = format!("[limits]\npage_bytes={budget}");
        let config = Config::parse(&text, None, None).unwrap();
        assert_eq!(config.limits.page_bytes, budget);
    }
    assert!(config.auth.is_none());
    assert_eq!(config.limits.shutdown_seconds, 30);
    for seconds in [1, 3600] {
        let text = format!("[limits]\nshutdown_seconds={seconds}");
        let config = Config::parse(&text, None, None).unwrap();
        assert_eq!(config.limits.shutdown_seconds, seconds);
    }
    let config = Config::parse(
        "listen='127.0.0.1:1'\n database_url='postgres://file/db'",
        Some("postgres://env/db".into()),
        Some("127.0.0.1:2".into()),
    )
    .unwrap();
    assert_eq!(config.listen.port(), 2);
    assert_eq!(config.database_url.unwrap(), "postgres://env/db");
    for text in [
        "extra=1",
        "allowed_schemas=[]",
        "allowed_schemas=['atlas_catalog']",
        "database_url='sqlite:///bad'",
        // The pool must keep at least one connection beyond the admitted data operations.
        "[limits]\npool_max=10\ndata_operations=10",
        "[limits]\nmax_limit=1",
        "[limits]\nquery_seconds=0",
        "[limits]\nshutdown_seconds=0",
        "[limits]\nshutdown_seconds=3601",
        "[limits]\nshutdown_seconds=-1",
        "[limits]\ntile_buffer=99999",
        "[limits]\nfilter_depth=129",
        "[limits]\nmax_limit=-1",
        "[limits]\nmax_limit=1.5",
        "[limits]\nextra=1",
        "[limits]\nmax_limit=1\nmax_limit=2",
        "[limits]\nexport_operations=0",
        "[limits]\nexport_operations=9",
        "[limits]\ndata_operations=1",
        "[limits]\ndata_operations=3\nexport_operations=4",
        "[limits]\nexport_operations=-1",
        "[limits]\npage_bytes=0",
        "[limits]\npage_bytes=1073741825",
        "[limits]\npage_bytes=-1",
        "[limits]\npage_bytes=4294967296",
    ] {
        assert!(Config::parse(text, None, None).is_err(), "{text}");
    }
    assert!(Config::parse("", None, Some("not-an-address".into())).is_err());
    for (text, data, export) in [
        ("[limits]\nexport_operations=1", 8, 1),
        ("[limits]\nexport_operations=8", 8, 8),
        ("[limits]\ndata_operations=2", 2, 2),
        ("[limits]\ndata_operations=3\nexport_operations=1", 3, 1),
        ("[limits]\ndata_operations=1\nexport_operations=1", 1, 1),
    ] {
        let limits = Config::parse(text, None, None).unwrap().limits;
        assert_eq!(
            (limits.data_operations, limits.export_operations),
            (data, export),
            "{text}"
        );
    }
    let mut config = Config::default();
    config.limits.export_operations = config.limits.data_operations + 1;
    assert!(config.validate().is_err());
}

#[test]
fn allowed_schemas_reject_system_schemas() {
    for schemas in [
        "['pg_catalog']",
        "['information_schema']",
        "['pg_toast']",
        "['pg_temp_1']",
        "['pg_toast_temp_1']",
        "['pg_']",
        "['pg_custom']",
        "['public','pg_catalog']",
        "['datos','information_schema']",
        "['public','atlas_catalog']",
    ] {
        let text = format!("allowed_schemas={schemas}");
        assert!(Config::parse(&text, None, None).is_err(), "{text}");
    }
    for schemas in ["['public','datos']", "['pgdatos']", "['datos_pg_catalog']"] {
        let text = format!("allowed_schemas={schemas}");
        let config = Config::parse(&text, None, None).unwrap_or_else(|e| panic!("{text}: {e:?}"));
        assert!(!config.allowed_schemas.is_empty());
    }
    let mut config = Config::parse("", None, None).unwrap();
    config.allowed_schemas = vec!["public".into(), "pg_catalog".into()];
    assert!(config.validate().is_err());
}

#[test]
fn pool_size_reserves_connections_beyond_data_operations() {
    // No fixed pool_max ceiling anymore; only data_operations < pool_max is required.
    for (text, pool) in [
        ("[limits]\npool_max=11", 11),
        ("[limits]\npool_max=20\ndata_operations=8", 20),
    ] {
        assert_eq!(
            Config::parse(text, None, None).unwrap().limits.pool_max,
            pool
        );
    }
}

#[tokio::test]
async fn jwt_trust_configuration_is_closed_and_local() {
    let text = "[auth]\npublic_key_file='tests/fixtures/jwt-test-public.pem'\nissuer='atlas-test'\naudience='atlas'";
    let config = Config::parse(text, None, None).unwrap();
    let auth = config.auth.unwrap();
    assert_eq!(auth.leeway_seconds, 30);
    assert!(auth.verifier().await.is_ok());
    for leeway in [0, 300] {
        let config =
            Config::parse(&format!("{text}\nleeway_seconds={leeway}"), None, None).unwrap();
        let auth = config.auth.unwrap();
        assert_eq!(auth.leeway_seconds, leeway);
        assert!(auth.verifier().await.is_ok());
    }
    for leeway in ["301", "-1", "1.5", "'30'", "99999999999"] {
        let text = format!("{text}\nleeway_seconds={leeway}");
        assert!(Config::parse(&text, None, None).is_err(), "{leeway}");
    }
    let strong = text.replace("jwt-test-public.pem", "jwt-test-4096-public.pem");
    let config = Config::parse(&strong, None, None).unwrap();
    assert!(config.auth.unwrap().verifier().await.is_ok());
    for invalid in [
        "[auth]",
        "[auth]\npublic_key_file='key.pem'",
        "[auth]\npublic_key_file=''\nissuer='i'\naudience='a'",
        "[auth]\npublic_key_file='k'\nissuer=' '\naudience='a'",
        "[auth]\npublic_key_file='k'\nissuer='i'\naudience=''",
    ] {
        assert!(Config::parse(invalid, None, None).is_err());
    }
    assert!(
        Config::parse(
            &format!("{text}\njwks_url='https://untrusted.invalid'"),
            None,
            None
        )
        .is_err()
    );
    for path in [
        "/missing/secret-key.pem",
        "tests/fixtures/jwt-test-private.pem",
        "tests/fixtures/jwt-test-1024-public.pem",
        "Cargo.toml",
    ] {
        let text = text.replace("tests/fixtures/jwt-test-public.pem", path);
        let config = Config::parse(&text, None, None).unwrap();
        assert!(config.auth.unwrap().verifier().await.is_err());
    }
}

#[tokio::test]
async fn jwks_trust_is_exclusive_and_fails_closed() {
    let base = "[auth]\nissuer='i'\naudience='a'";
    for invalid in [
        format!("{base}\njwks_url=''"),
        format!("{base}\njwks_url='https://idp.test/jwks'\npublic_key_file='k'"),
        format!("{base}\njwks_url='https://idp.test/jwks'\nroles_claim='a..b'"),
        format!("{base}\npublic_key_file='k'\nroles_claim=''"),
    ] {
        assert!(Config::parse(&invalid, None, None).is_err(), "{invalid}");
    }
    let config = Config::parse(&format!("{base}\npublic_key_file='k'"), None, None).unwrap();
    assert_eq!(config.auth.unwrap().roles_claim, "roles");
    for url in ["http://idp.example/jwks", "http://127.0.0.1:9/jwks"] {
        let text = format!("{base}\njwks_url='{url}'\nroles_claim='realm_access.roles'");
        let config = Config::parse(&text, None, None).unwrap();
        assert!(config.auth.unwrap().verifier().await.is_err());
    }
}

#[test]
fn cors_origins_are_disabled_by_default_and_strictly_validated() {
    let cors =
        |origins: &str| Config::parse(&format!("[cors]\nallowed_origins={origins}"), None, None);
    assert!(
        Config::parse("", None, None)
            .unwrap()
            .cors
            .allowed_origins
            .is_empty()
    );
    assert!(cors("[]").unwrap().cors.allowed_origins.is_empty());
    for valid in [
        "['*']",
        "['https://a.example']",
        "['http://a.example:8080', 'https://b.example']",
    ] {
        assert!(cors(valid).is_ok(), "{valid}");
    }
    let list = |n: usize| {
        let origins: Vec<_> = (0..n).map(|i| format!("'https://h{i}.example'")).collect();
        format!("[{}]", origins.join(","))
    };
    assert_eq!(cors(&list(100)).unwrap().cors.allowed_origins.len(), 100);
    for invalid in [
        "['*','https://a.example']".to_owned(),
        "['https://a.example/']".into(),
        "['ftp://x']".into(),
        "['https://a.example','https://a.example']".into(),
        list(101),
        "['https://a.example/path']".into(),
        "['https://a.example?q=1']".into(),
        "['https://a.example#f']".into(),
        "[' https://a.example']".into(),
        "['https://a.example ']".into(),
        "['https://a .example']".into(),
        "['a.example']".into(),
        "['']".into(),
        "'https://a.example'".into(),
    ] {
        assert!(cors(&invalid).is_err(), "{invalid}");
    }
}
