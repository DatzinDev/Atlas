use atlas::{
    auth::policy::{admin, download, query, view},
    domain::{Dataset, Error, Principal},
};
use serde_json::json;

fn dataset() -> Dataset {
    serde_json::from_value(json!({"id":"places", "title":"Places",
        "source":{"schema":"public", "table":"places"}, "properties":["name"],
        "access":{"view_roles":["view"], "query_roles":["query"], "download_roles":["download"]}}))
    .unwrap()
}

#[test]
fn permission_matrix_is_closed_and_operations_require_visibility() {
    for public in [false, true] {
        for public_download in [false, true] {
            for enabled in [false, true] {
                for authenticated in [false, true] {
                    for bits in 0..16 {
                        let mut dataset = dataset();
                        dataset.enabled = enabled;
                        dataset.access.public = public;
                        dataset.access.public_download = public_download;
                        let principal = Principal {
                            sub: authenticated.then(|| "alice".into()),
                            roles: ["view", "query", "download", "atlas-admin"]
                                .into_iter()
                                .enumerate()
                                .filter(|(i, _)| bits & (1 << i) != 0)
                                .map(|(_, role)| role.into())
                                .collect(),
                        };
                        let visible = enabled && (public || (authenticated && bits & 1 != 0));
                        assert_eq!(view(&dataset, &principal).is_ok(), visible);
                        for (result, granted) in [
                            (
                                query(&dataset, &principal),
                                public || (authenticated && bits & 2 != 0),
                            ),
                            (
                                download(&dataset, &principal),
                                (public && public_download) || (authenticated && bits & 4 != 0),
                            ),
                        ] {
                            assert_eq!(
                                result,
                                if !visible {
                                    Err(Error::DatasetNotFound)
                                } else if !granted {
                                    Err(Error::PermissionDenied)
                                } else {
                                    Ok(())
                                }
                            );
                        }
                        assert_eq!(admin(&principal).is_ok(), authenticated && bits & 8 != 0);
                    }
                }
            }
        }
    }
}

#[test]
fn catalog_visibility_filters_before_pagination_and_predicates_cannot_grant_access() {
    let anonymous = Principal::default();
    let administrator = Principal {
        sub: Some("admin".into()),
        roles: vec!["atlas-admin".into()],
    };
    let mut hidden = dataset();
    hidden.id = "a-hidden".into();
    hidden.filter = Some(
        serde_json::value::RawValue::from_string(
            r#"{"op":"or","args":[{"op":"not","arg":{"op":"eq","field":"name","value":"x"}}]}"#
                .into(),
        )
        .unwrap(),
    );
    let mut public = dataset();
    public.id = "b-public".into();
    public.access.public = true;
    let mut disabled = public.clone();
    disabled.id = "c-disabled".into();
    disabled.enabled = false;
    let datasets = [hidden, public, disabled];
    for principal in [&anonymous, &administrator] {
        let ids: Vec<_> = datasets
            .iter()
            .filter(|d| view(d, principal).is_ok())
            .take(1)
            .map(|d| d.id.as_str())
            .collect();
        assert_eq!(ids, ["b-public"]);
        let mut source_reads = 0;
        let result = query(&datasets[0], principal).map(|()| {
            source_reads += 1;
        });
        assert_eq!(result, Err(Error::DatasetNotFound));
        assert_eq!(source_reads, 0);
        assert_eq!(
            download(&datasets[1], principal),
            Err(Error::PermissionDenied)
        );
        assert_eq!(query(&datasets[2], principal), Err(Error::DatasetNotFound));
    }
    let mut blank = administrator;
    blank.sub = Some(" ".into());
    assert_eq!(admin(&blank), Err(Error::PermissionDenied));
}
