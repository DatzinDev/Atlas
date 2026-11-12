use atlas::{
    auth::jwks::{Jwks, MAX_JWKS_BYTES, Timing},
    domain::Error,
};
use axum::{Router, extract::State, http::StatusCode, routing::get};
use jsonwebtoken::{
    Algorithm, EncodingKey,
    jwk::{AlgorithmParameters, Jwk, JwkSet, PublicKeyUse},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

const FIRST: &[u8] = include_bytes!("fixtures/jwt-test-private.pem");

#[derive(Default)]
struct Provider {
    response: Mutex<(u16, String, u64)>,
    hits: AtomicUsize,
}

impl Provider {
    fn set(&self, status: u16, body: String, delay_ms: u64) {
        *self.response.lock().unwrap() = (status, body, delay_ms);
    }
}

async fn jwks(State(provider): State<Arc<Provider>>) -> (StatusCode, String) {
    provider.hits.fetch_add(1, Ordering::SeqCst);
    let (status, body, delay) = provider.response.lock().unwrap().clone();
    tokio::time::sleep(Duration::from_millis(delay)).await;
    (StatusCode::from_u16(status).unwrap(), body)
}

fn jwk(pem: &[u8], kid: &str, key_use: Option<PublicKeyUse>) -> Jwk {
    let key = EncodingKey::from_rsa_pem(pem).unwrap();
    let mut jwk = Jwk::from_encoding_key(&key, Algorithm::RS256).unwrap();
    jwk.common.key_id = Some(kid.into());
    jwk.common.public_key_use = key_use;
    jwk
}

fn set(keys: Vec<Jwk>) -> String {
    serde_json::to_string(&JwkSet { keys }).unwrap()
}

fn first() -> String {
    set(vec![jwk(FIRST, "first", None)])
}

fn timing() -> Timing {
    Timing {
        timeout: Duration::from_millis(500),
    }
}

async fn start(body: String) -> (Arc<Provider>, String) {
    let provider = Arc::new(Provider::default());
    provider.set(200, body, 0);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/jwks", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/jwks", get(jwks))
        .with_state(provider.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (provider, url)
}

#[tokio::test]
async fn loads_only_trusted_signing_keys_and_fails_closed_at_startup() {
    let (provider, url) = start(set(vec![
        jwk(FIRST, "enc", Some(PublicKeyUse::Encryption)),
        jwk(FIRST, "first", Some(PublicKeyUse::Signature)),
    ]))
    .await;
    let jwks = Jwks::load(&url, timing()).await.unwrap();
    assert!(jwks.key("first").await.is_ok());
    assert_eq!(jwks.key("enc").await.err(), Some("unknown_kid"));
    let mut weak = jwk(FIRST, "weak", None);
    let AlgorithmParameters::RSA(rsa) = &mut weak.algorithm else {
        unreachable!()
    };
    rsa.n.truncate(171);
    provider.set(200, set(vec![weak, jwk(FIRST, "first", None)]), 0);
    let jwks = Jwks::load(&url, timing()).await.unwrap();
    assert_eq!(jwks.key("weak").await.err(), Some("unknown_kid"));
    let mut anonymous = jwk(FIRST, "", None);
    anonymous.common.key_id = None;
    for (status, body, delay) in [
        (500, first(), 0),
        (302, String::new(), 0),
        (200, first(), 2000),
        (200, "x".repeat(MAX_JWKS_BYTES + 1), 0),
        (200, "{\"keys\":[]}".into(), 0),
        (
            200,
            set(vec![jwk(FIRST, "enc", Some(PublicKeyUse::Encryption))]),
            0,
        ),
        (200, set(vec![anonymous]), 0),
    ] {
        provider.set(status, body, delay);
        assert!(matches!(
            Jwks::load(&url, timing()).await,
            Err(Error::InvalidRequest)
        ));
    }
    provider.set(200, first(), 0);
    for untrusted in ["http://idp.example/jwks", "ftp://127.0.0.1/jwks", "jwks"] {
        assert!(matches!(
            Jwks::load(untrusted, timing()).await,
            Err(Error::InvalidRequest)
        ));
    }
}
