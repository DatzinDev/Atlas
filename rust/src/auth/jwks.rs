use super::jwt::usable;
use crate::{api::log, domain::Error};
use axum::{body::Bytes, http::Uri};
use http_body_util::{BodyExt, Empty, Limited};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use jsonwebtoken::{
    DecodingKey,
    jwk::{AlgorithmParameters, JwkSet, KeyAlgorithm, PublicKeyUse},
};
use serde_json::json;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};

pub const MAX_JWKS_BYTES: usize = 65536;
pub const MAX_KEYS: usize = 32;

/// Fixed in production by ADR 0010; tests shorten them.
#[derive(Clone, Copy)]
pub struct Timing {
    pub min_refresh: Duration,
    pub max_age: Duration,
    pub max_stale: Duration,
    pub timeout: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            min_refresh: Duration::from_secs(60),
            max_age: Duration::from_secs(3600),
            max_stale: Duration::from_secs(86400),
            timeout: Duration::from_secs(5),
        }
    }
}

struct Keys {
    keys: HashMap<String, DecodingKey>,
    fetched: Instant,
    attempted: Instant,
    failed: bool,
}

/// In-process cache of the operator JWKS public keys; never stores tokens or claims.
pub struct Jwks {
    url: Uri,
    timing: Timing,
    client: Client<HttpsConnector<HttpConnector>, Empty<Bytes>>,
    keys: RwLock<Keys>,
    refresh: Mutex<()>,
}

/// HTTPS, or plain HTTP only towards loopback for tests.
fn trusted_url(url: &str) -> Option<Uri> {
    let uri: Uri = url.parse().ok()?;
    let loopback = matches!(uri.host()?, "localhost" | "127.0.0.1" | "[::1]");
    match uri.scheme_str()? {
        "https" => Some(uri),
        "http" if loopback => Some(uri),
        _ => None,
    }
}

/// Strong RSA signing keys with a `kid`; anything else in the set is ignored.
fn signing_keys(set: JwkSet) -> HashMap<String, DecodingKey> {
    set.keys
        .iter()
        .filter(|jwk| matches!(jwk.algorithm, AlgorithmParameters::RSA(_)))
        .filter(|jwk| {
            matches!(
                jwk.common.public_key_use,
                None | Some(PublicKeyUse::Signature)
            )
        })
        .filter(|jwk| matches!(jwk.common.key_algorithm, None | Some(KeyAlgorithm::RS256)))
        .filter_map(|jwk| {
            let key = DecodingKey::from_jwk(jwk).ok()?;
            usable(&key)?;
            Some((jwk.common.key_id.clone()?, key))
        })
        .take(MAX_KEYS)
        .collect()
}

impl Jwks {
    /// Fails closed when the first download yields no usable key.
    pub async fn load(url: &str, timing: Timing) -> Result<Self, Error> {
        let url = trusted_url(url).ok_or(Error::InvalidRequest)?;
        let connector = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();
        let now = Instant::now();
        let jwks = Self {
            url,
            timing,
            client: Client::builder(TokioExecutor::new()).build(connector),
            keys: RwLock::new(Keys {
                keys: HashMap::new(),
                fetched: now,
                attempted: now,
                failed: false,
            }),
            refresh: Mutex::new(()),
        };
        jwks.keys.write().await.keys = jwks.fetch().await.map_err(|_| Error::InvalidRequest)?;
        Ok(jwks)
    }

    async fn download(&self) -> Result<HashMap<String, DecodingKey>, &'static str> {
        let response = self
            .client
            .get(self.url.clone())
            .await
            .map_err(|_| "request")?;
        if !response.status().is_success() {
            return Err("status");
        }
        let body = Limited::new(response.into_body(), MAX_JWKS_BYTES)
            .collect()
            .await
            .map_err(|_| "body")?
            .to_bytes();
        let set = serde_json::from_slice::<JwkSet>(&body).map_err(|_| "invalid")?;
        Some(signing_keys(set))
            .filter(|keys| !keys.is_empty())
            .ok_or("no_keys")
    }

    /// Logs a stable reason on failure; never the URL or the body.
    async fn fetch(&self) -> Result<HashMap<String, DecodingKey>, ()> {
        let result = tokio::time::timeout(self.timing.timeout, self.download())
            .await
            .unwrap_or(Err("timeout"));
        result.map_err(|reason| log(json!({"event": "jwks_refresh_failed", "reason": reason})))
    }

    /// One download at a time, at most one per `min_refresh`.
    async fn renew(&self, kid: &str) {
        let _single = self.refresh.lock().await;
        {
            let keys = self.keys.read().await;
            let fresh = keys.fetched.elapsed() < self.timing.max_age;
            if (fresh && keys.keys.contains_key(kid))
                || keys.attempted.elapsed() < self.timing.min_refresh
            {
                return;
            }
        }
        self.keys.write().await.attempted = Instant::now();
        let result = self.fetch().await;
        let mut keys = self.keys.write().await;
        keys.failed = result.is_err();
        if let Ok(fetched) = result {
            keys.keys = fetched;
            keys.fetched = Instant::now();
        }
    }

    /// On failure returns the stable reason logged for the 401.
    pub async fn key(&self, kid: &str) -> Result<DecodingKey, &'static str> {
        {
            let keys = self.keys.read().await;
            if keys.fetched.elapsed() < self.timing.max_age
                && let Some(key) = keys.keys.get(kid)
            {
                return Ok(key.clone());
            }
        }
        self.renew(kid).await;
        let keys = self.keys.read().await;
        if keys.fetched.elapsed() >= self.timing.max_stale {
            return Err("jwks_expired");
        }
        match keys.keys.get(kid) {
            Some(key) => Ok(key.clone()),
            None if keys.failed => Err("jwks_unavailable"),
            None => Err("unknown_kid"),
        }
    }
}
