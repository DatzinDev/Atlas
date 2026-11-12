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
use std::{collections::HashMap, time::Duration};

pub const MAX_JWKS_BYTES: usize = 65536;
pub const MAX_KEYS: usize = 32;

/// Fixed in production by ADR 0010; tests shorten them.
#[derive(Clone, Copy)]
pub struct Timing {
    pub timeout: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
        }
    }
}

/// In-process cache of the operator JWKS public keys; never stores tokens or claims.
pub struct Jwks {
    url: Uri,
    timing: Timing,
    client: Client<HttpsConnector<HttpConnector>, Empty<Bytes>>,
    keys: HashMap<String, DecodingKey>,
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
        let mut jwks = Self {
            url,
            timing,
            client: Client::builder(TokioExecutor::new()).build(connector),
            keys: HashMap::new(),
        };
        jwks.keys = jwks.fetch().await.map_err(|_| Error::InvalidRequest)?;
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

    /// On failure returns the stable reason logged for the 401.
    pub async fn key(&self, kid: &str) -> Result<DecodingKey, &'static str> {
        self.keys.get(kid).cloned().ok_or("unknown_kid")
    }
}
