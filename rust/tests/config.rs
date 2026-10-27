use atlas::config::Config;

#[test]
fn defaults_and_overrides_are_validated() {
    let config = Config::parse("", None, None).unwrap();
    assert_eq!(config.limits.max_limit, 10000);
    assert_eq!(config.limits.pool_max, 10);
    assert_eq!(config.limits.export_bytes, 268435456);
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
