use atlas::{
    auth::{
        jwks::{Jwks, MAX_JWKS_BYTES, Timing},
        jwt::Verifier,
    },
    domain::Error,
};
use axum::{Router, extract::State, http::StatusCode, routing::get};
use jsonwebtoken::{
    Algorithm, EncodingKey, Header, encode, get_current_timestamp,
    jwk::{AlgorithmParameters, Jwk, JwkSet, PublicKeyUse},
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

const FIRST: &[u8] = include_bytes!("fixtures/jwt-test-private.pem");
const ROTATED: &[u8] = include_bytes!("fixtures/oidc-test-rotated.pem");

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
        min_refresh: Duration::ZERO,
        timeout: Duration::from_millis(500),
        ..Timing::default()
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

fn token(pem: &[u8], kid: Option<&str>, change: impl FnOnce(&mut Value)) -> String {
    let mut claims = json!({"sub":"alice", "iss":"https://idp.test", "aud":"atlas",
        "exp":get_current_timestamp()+3600, "realm_access":{"roles":["reader"]}});
    change(&mut claims);
    let mut header = Header::new(Algorithm::RS256);
    header.kid = kid.map(str::to_owned);
    encode(&header, &claims, &EncodingKey::from_rsa_pem(pem).unwrap()).unwrap()
}

#[tokio::test]
async fn loads_only_trusted_signing_keys_and_fails_closed_at_startup() {
    let (provider, url) = start(set(vec![
        jwk(ROTATED, "enc", Some(PublicKeyUse::Encryption)),
        jwk(FIRST, "first", Some(PublicKeyUse::Signature)),
    ]))
    .await;
    let jwks = Jwks::load(&url, timing()).await.unwrap();
    assert!(jwks.key("first").await.is_ok());
    assert_eq!(jwks.key("enc").await.err(), Some("unknown_kid"));
    let mut weak = jwk(ROTATED, "weak", None);
    let AlgorithmParameters::RSA(rsa) = &mut weak.algorithm else {
        unreachable!()
    };
    let mut exponent_one = rsa.clone();
    exponent_one.e = "AQ".into();
    rsa.n.truncate(171);
    let mut one = jwk(ROTATED, "one", None);
    one.algorithm = AlgorithmParameters::RSA(exponent_one);
    provider.set(200, set(vec![weak, one, jwk(FIRST, "first", None)]), 0);
    let jwks = Jwks::load(&url, timing()).await.unwrap();
    for kid in ["weak", "one"] {
        assert_eq!(jwks.key(kid).await.err(), Some("unknown_kid"));
    }
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

#[tokio::test]
async fn rotates_by_kid_with_a_single_bounded_refresh() {
    let (provider, url) = start(first()).await;
    let limited = Timing {
        min_refresh: Duration::from_secs(3600),
        ..timing()
    };
    let jwks = Jwks::load(&url, limited).await.unwrap();
    for _ in 0..3 {
        assert_eq!(jwks.key("missing").await.err(), Some("unknown_kid"));
    }
    assert_eq!(provider.hits.load(Ordering::SeqCst), 1);
    let (provider, url) = start(first()).await;
    let jwks = Arc::new(Jwks::load(&url, timing()).await.unwrap());
    provider.set(200, set(vec![jwk(ROTATED, "rotated", None)]), 100);
    let checks: Vec<_> = (0..8)
        .map(|_| {
            let jwks = jwks.clone();
            tokio::spawn(async move { jwks.key("rotated").await.is_ok() })
        })
        .collect();
    for check in checks {
        assert!(check.await.unwrap());
    }
    assert_eq!(provider.hits.load(Ordering::SeqCst), 2);
    assert!(jwks.key("first").await.is_err());
}

#[tokio::test]
async fn provider_outage_keeps_keys_until_stale_then_fails_closed() {
    let (provider, url) = start(first()).await;
    let short = Timing {
        max_age: Duration::ZERO,
        max_stale: Duration::from_secs(1),
        ..timing()
    };
    let jwks = Jwks::load(&url, short).await.unwrap();
    provider.set(503, String::new(), 0);
    assert!(jwks.key("first").await.is_ok());
    assert_eq!(jwks.key("other").await.err(), Some("jwks_unavailable"));
    provider.set(200, first(), 2000);
    assert!(jwks.key("first").await.is_ok());
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(jwks.key("first").await.err(), Some("jwks_expired"));
    provider.set(200, first(), 0);
    assert!(jwks.key("first").await.is_ok());
}

#[tokio::test]
async fn verifies_tokens_with_mapped_roles() {
    let (_, url) = start(first()).await;
    let jwks = || Jwks::load(&url, timing());
    for (issuer, audience) in [(" ", "atlas"), ("i", "")] {
        assert!(Verifier::from_jwks(jwks().await.unwrap(), issuer, audience).is_err());
    }
    let verifier = || async {
        Verifier::from_jwks(jwks().await.unwrap(), "https://idp.test", "atlas").unwrap()
    };
    for roles in ["", "a..b", ".roles"] {
        assert!(verifier().await.with_roles_claim(roles).is_err());
    }
    let oidc = verifier()
        .await
        .with_roles_claim("realm_access.roles")
        .unwrap();
    let principal = oidc
        .verify(&token(FIRST, Some("first"), |_| {}))
        .await
        .unwrap();
    assert_eq!(principal.sub.as_deref(), Some("alice"));
    assert_eq!(principal.roles, ["reader"]);
    let none = token(FIRST, Some("first"), |c| c["realm_access"] = json!({}));
    assert!(oidc.verify(&none).await.unwrap().roles.is_empty());
    let late = token(FIRST, Some("first"), |c| {
        c["exp"] = json!(get_current_timestamp() - 20)
    });
    assert!(oidc.verify(&late).await.is_ok());
    let strict = verifier().await.with_leeway(0);
    assert_eq!(strict.verify(&late).await.err(), Some("expired"));
    assert_eq!(
        oidc.verify(&token(FIRST, None, |_| {})).await.err(),
        Some("missing_kid")
    );
    assert_eq!(
        oidc.verify(&token(FIRST, Some("nope"), |_| {})).await.err(),
        Some("unknown_kid")
    );
    let hs = encode(
        &Header {
            kid: Some("first".into()),
            ..Header::new(Algorithm::HS256)
        },
        &json!({"sub":"alice","iss":"https://idp.test","aud":"atlas","exp":get_current_timestamp()+60}),
        &EncodingKey::from_secret(b"first"),
    )
    .unwrap();
    for bad in [
        hs,
        token(FIRST, Some("first"), |c| {
            c["iss"] = json!("https://other.test")
        }),
        token(FIRST, Some("first"), |c| c["aud"] = json!("other")),
        token(FIRST, Some("first"), |c| {
            c["exp"] = json!(get_current_timestamp() - 60)
        }),
        token(FIRST, Some("first"), |c| {
            c["nbf"] = json!(get_current_timestamp() + 60)
        }),
        token(FIRST, Some("first"), |c| c["sub"] = json!(" ")),
        token(FIRST, Some("first"), |c| {
            c["realm_access"]["roles"] = json!("reader")
        }),
        token(FIRST, Some("first"), |c| {
            c["realm_access"]["roles"] = json!([" "])
        }),
        token(ROTATED, Some("first"), |_| {}),
    ] {
        assert!(oidc.verify(&bad).await.is_err());
    }
}
