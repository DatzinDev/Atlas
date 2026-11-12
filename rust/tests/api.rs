use atlas::{
    api::{failure, log},
    domain::Error,
};
use axum::body::to_bytes;
use std::collections::HashSet;

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

#[tokio::test]
async fn failures_share_one_id_generator() {
    let mut ids = HashSet::new();
    let mut nonces = HashSet::new();
    for error in [Error::NotFound, Error::InvalidToken, Error::QueryTooLarge]
        .into_iter()
        .cycle()
        .take(100)
    {
        let response = failure(error);
        let status = response.status();
        let header = response
            .headers()
            .get("x-request-id")
            .map(|v| v.to_str().unwrap().to_owned());
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = body["request_id"].as_str().unwrap().to_owned();
        assert!(well_formed(&id), "{id}");
        if let Some(header) = header {
            assert_eq!(header, id);
        }
        assert!(status.is_client_error());
        nonces.insert(id.split_once('-').unwrap().0.to_owned());
        assert!(ids.insert(id.clone()), "repeated {id}");
    }
    assert_eq!(ids.len(), 100);
    assert_eq!(nonces.len(), 1, "nonce is fixed per process: {nonces:?}");
}

#[test]
fn log_accepts_any_json_value() {
    log(serde_json::json!({"event": "test", "detail": "line\nbreak"}));
}
