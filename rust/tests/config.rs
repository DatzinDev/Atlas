use atlas::config::Config;

#[test]
fn defaults_and_overrides_are_validated() {
    let config = Config::parse("", None, None).unwrap();
    assert_eq!(config.limits.max_limit, 10000);
    assert_eq!(config.limits.pool_max, 10);
    assert_eq!(config.limits.export_bytes, 268435456);
    assert!(config.auth.is_none());
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
        "[limits]\npool_max=11",
        "[limits]\nmax_limit=1",
        "[limits]\nquery_seconds=0",
        "[limits]\ntile_buffer=99999",
        "[limits]\nfilter_depth=129",
        "[limits]\nmax_limit=-1",
        "[limits]\nmax_limit=1.5",
        "[limits]\nextra=1",
        "[limits]\nmax_limit=1\nmax_limit=2",
    ] {
        assert!(Config::parse(text, None, None).is_err(), "{text}");
    }
    assert!(Config::parse("", None, Some("not-an-address".into())).is_err());
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
fn jwt_trust_configuration_is_closed_and_local() {
    let text = "[auth]\npublic_key_file='tests/fixtures/jwt-test-public.pem'\nissuer='atlas-test'\naudience='atlas'";
    let config = Config::parse(text, None, None).unwrap();
    let auth = config.auth.unwrap();
    assert_eq!(auth.leeway_seconds, 30);
    assert!(auth.verifier().is_ok());
    for leeway in [0, 300] {
        let config =
            Config::parse(&format!("{text}\nleeway_seconds={leeway}"), None, None).unwrap();
        let auth = config.auth.unwrap();
        assert_eq!(auth.leeway_seconds, leeway);
        assert!(auth.verifier().is_ok());
    }
    for leeway in ["301", "-1", "1.5", "'30'", "99999999999"] {
        let text = format!("{text}\nleeway_seconds={leeway}");
        assert!(Config::parse(&text, None, None).is_err(), "{leeway}");
    }
    let strong = text.replace("jwt-test-public.pem", "jwt-test-4096-public.pem");
    let config = Config::parse(&strong, None, None).unwrap();
    assert!(config.auth.unwrap().verifier().is_ok());
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
        assert!(config.auth.unwrap().verifier().is_err());
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
