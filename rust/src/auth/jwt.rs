use crate::domain::{Error, Principal};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, errors::ErrorKind};
use serde::Deserialize;

#[derive(Deserialize)]
struct Claims {
    sub: String,
    #[serde(rename = "iss")]
    _issuer: String,
    #[serde(rename = "aud")]
    _audience: serde_json::Value,
    #[serde(rename = "exp")]
    _expires: u64,
    #[serde(default, deserialize_with = "present_timestamp", rename = "nbf")]
    _not_before: Option<u64>,
    #[serde(default)]
    roles: Vec<String>,
}

fn present_timestamp<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    u64::deserialize(d).map(Some)
}

fn reason(kind: &ErrorKind) -> &'static str {
    match kind {
        ErrorKind::InvalidSignature => "invalid_signature",
        ErrorKind::InvalidAlgorithm
        | ErrorKind::MissingAlgorithm
        | ErrorKind::UnsupportedAlgorithm
        | ErrorKind::InvalidAlgorithmName => "invalid_algorithm",
        ErrorKind::ExpiredSignature => "expired",
        ErrorKind::ImmatureSignature => "not_yet_valid",
        ErrorKind::InvalidIssuer => "invalid_issuer",
        ErrorKind::InvalidAudience => "invalid_audience",
        ErrorKind::MissingRequiredClaim(_)
        | ErrorKind::InvalidClaimFormat(_)
        | ErrorKind::InvalidSubject
        | ErrorKind::Json(_) => "invalid_claims",
        _ => "malformed_token",
    }
}

pub struct Verifier {
    key: DecodingKey,
    validation: Validation,
}

impl Verifier {
    pub fn new(pem: &[u8], issuer: &str, audience: &str) -> Result<Self, Error> {
        let pem = pem.trim_ascii();
        if issuer.trim().is_empty()
            || audience.trim().is_empty()
            || !(pem.starts_with(b"-----BEGIN PUBLIC KEY-----")
                || pem.starts_with(b"-----BEGIN RSA PUBLIC KEY-----"))
        {
            return Err(Error::InvalidRequest);
        }
        let key = DecodingKey::from_rsa_pem(pem).map_err(|_| Error::InvalidRequest)?;
        (jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER.verifier_factory)(&Algorithm::RS256, &key)
            .map_err(|_| Error::InvalidRequest)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["sub", "iss", "aud", "exp"]);
        validation.set_issuer(&[issuer]);
        validation.set_audience(&[audience]);
        validation.validate_nbf = true;
        validation.leeway = 0;
        validation.reject_tokens_expiring_in_less_than = 1;
        Ok(Self { key, validation })
    }

    /// On failure returns a stable reason for logs; never the token or its claims.
    pub fn verify(&self, token: &str) -> Result<Principal, &'static str> {
        // Bound decoding and signature work independently of future request body limits.
        if token.len() > 16384 {
            return Err("token_too_large");
        }
        let claims = decode::<Claims>(token, &self.key, &self.validation)
            .map_err(|e| reason(e.kind()))?
            .claims;
        if claims.sub.trim().is_empty() || claims.roles.iter().any(|r| r.trim().is_empty()) {
            return Err("invalid_claims");
        }
        Ok(Principal {
            sub: Some(claims.sub),
            roles: claims.roles,
        })
    }
}
