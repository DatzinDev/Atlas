use atlas::domain::{Dataset, Error, Query};

#[test]
fn closed_models_and_private_defaults() {
    let json = r#"{"id":"roads","title":"Roads","source":{"schema":"public","table":"roads"},"properties":["id"]}"#;
    let mut dataset: Dataset = serde_json::from_str(json).unwrap();
    assert!(!dataset.access.public && !dataset.access.public_download);
    assert!(dataset.enabled);
    dataset.validate(&["public".into()]).unwrap();
    assert!(dataset.validate(&["other".into()]).is_err());
    dataset.properties.push("id".into());
    assert_eq!(
        dataset.validate(&["public".into()]),
        Err(Error::InvalidRequest)
    );
    for invalid in [
        json.replace("roads\",\"title", "ROAD\",\"title"),
        json.replace("\"id\"]", "\"\"]"),
    ] {
        assert!(
            serde_json::from_str::<Dataset>(&invalid)
                .unwrap()
                .validate(&["public".into()])
                .is_err()
        );
    }
    for invalid in [
        r#"{"extra":1}"#,
        r#"{"limit":1,"limit":2}"#,
        r#"{"limit":true}"#,
        r#"{"sort":[{"field":"id","direction":"ASC"}]}"#,
        r#"{"filter":null,"bbox":[0,0,1e999,1]}"#,
    ] {
        assert!(serde_json::from_str::<Query>(invalid).is_err());
    }
}

#[test]
fn access_roles_are_bounded() {
    let base = r#"{"id":"roads","title":"Roads","source":{"schema":"public","table":"roads"},"properties":["id"]}"#;
    let allowed = ["public".to_owned()];
    let many = |n: usize| (0..n).map(|i| format!("user:{i}")).collect::<Vec<_>>();
    for list in 0..3 {
        let check = |roles: Vec<String>| {
            let mut dataset: Dataset = serde_json::from_str(base).unwrap();
            let access = &mut dataset.access;
            *[
                &mut access.view_roles,
                &mut access.query_roles,
                &mut access.download_roles,
            ][list] = roles;
            dataset.validate(&allowed)
        };
        for valid in [
            many(1000),
            vec![
                "share:roads".into(),
                "/geo/editors".into(),
                "a@b.example".into(),
            ],
            vec!["x".repeat(128)],
            vec!["é".repeat(128)],
            vec!["\u{1F600}".repeat(128)],
        ] {
            assert_eq!(check(valid), Ok(()));
        }
        for invalid in [
            many(1001),
            vec![String::new()],
            vec![" viewer".into()],
            vec!["view er".into()],
            vec!["view\ter".into()],
            vec!["view\u{A0}er".into()],
            vec!["\u{FEFF}viewer".into()],
            vec!["viewer\u{1}".into()],
            vec!["x".repeat(129)],
            vec!["é".repeat(129)],
            vec!["\u{1F600}".repeat(129)],
            vec!["view\u{85}er".into()],
            vec!["view\u{2028}er".into()],
        ] {
            assert_eq!(check(invalid), Err(Error::InvalidRequest));
        }
    }
}

#[test]
fn query_validation_preserves_numeric_filter() {
    for json in [
        r#"{"limit":0}"#,
        r#"{"offset":100001}"#,
        r#"{"bbox":[2,0,1,1]}"#,
        r#"{"properties":[]}"#,
        r#"{"sort":[{"field":"id","direction":"asc"},{"field":"id","direction":"desc"}]}"#,
    ] {
        let query: Query = serde_json::from_str(json).unwrap();
        assert!(query.validate(10000, 100000).is_err());
    }
    let raw = r#"{"op":"eq","field":"cost","value":12345678901234567890.123456789}"#;
    let query: Query = serde_json::from_str(&format!("{{\"filter\":{raw}}}")).unwrap();
    query.validate(10000, 100000).unwrap();
    assert_eq!(query.filter.unwrap().get(), raw);
    let number: serde_json::Value = serde_json::from_str("12345678901234567890.123456789").unwrap();
    assert_eq!(number.to_string(), "12345678901234567890.123456789");
    assert_eq!(
        Error::DatabaseUnavailable.to_string(),
        "DatabaseUnavailable"
    );
}
