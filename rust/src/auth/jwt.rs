use crate::domain::{Error, Principal};
use jsonwebtoken::{
    Algorithm, DecodingKey, DecodingKeyKind, Validation, decode, errors::ErrorKind,
};
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

/// Clock skew tolerance applied to `exp` and `nbf` unless configured otherwise.
pub const DEFAULT_LEEWAY_SECONDS: u64 = 30;

/// Splits one DER element with the expected tag into (contents, remainder).
fn der(input: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&found, rest) = input.split_first()?;
    let (&first, rest) = rest.split_first()?;
    if found != tag {
        return None;
    }
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        // Two length bytes cover moduli far beyond 8192 bits.
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 2 || rest.len() < count {
            return None;
        }
        let (bytes, rest) = rest.split_at(count);
        let len = bytes.iter().fold(0, |acc, &b| (acc << 8) | usize::from(b));
        (len, rest)
    };
    (rest.len() >= len).then(|| rest.split_at(len))
}

/// Positive DER INTEGER without leading zero bytes.
fn unsigned(int: &[u8]) -> Option<&[u8]> {
    if int.first()? & 0x80 != 0 {
        return None;
    }
    int.get(int.iter().position(|&b| b != 0).unwrap_or(int.len())..)
}

/// Accepts PKCS#1 `RSAPublicKey` DER with a 2048..=8192-bit modulus and odd exponent >= 3.
fn strong_rsa(key: &DecodingKey) -> Option<()> {
    let DecodingKeyKind::SecretOrDer(bytes) = key.kind() else {
        return None;
    };
    let (body, rest) = der(bytes, 0x30)?;
    let (n, body) = der(body, 0x02)?;
    let (e, body) = der(body, 0x02)?;
    let (n, e) = (unsigned(n)?, unsigned(e)?);
    let bits = n.len() * 8 - n.first()?.leading_zeros() as usize;
    let odd = e.last()? & 1 == 1;
    (rest.is_empty()
        && body.is_empty()
        && (2048..=8192).contains(&bits)
        && odd
        && (e.len() > 1 || e[0] >= 3))
        .then_some(())
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
        strong_rsa(&key).ok_or(Error::InvalidRequest)?;
        (jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER.verifier_factory)(&Algorithm::RS256, &key)
            .map_err(|_| Error::InvalidRequest)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["sub", "iss", "aud", "exp"]);
        validation.set_issuer(&[issuer]);
        validation.set_audience(&[audience]);
        validation.validate_nbf = true;
        validation.leeway = DEFAULT_LEEWAY_SECONDS;
        validation.reject_tokens_expiring_in_less_than = 1;
        Ok(Self { key, validation })
    }

    /// Overrides the `exp`/`nbf` clock skew tolerance; range is enforced by configuration.
    pub fn with_leeway(mut self, seconds: u64) -> Self {
        self.validation.leeway = seconds;
        self
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
