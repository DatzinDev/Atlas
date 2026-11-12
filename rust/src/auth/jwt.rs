use super::jwks::Jwks;
use crate::domain::{Error, Principal};
use jsonwebtoken::{
    Algorithm, DecodingKey, DecodingKeyKind, Validation, decode, decode_header, errors::ErrorKind,
};
use serde::Deserialize;
use serde_json::value::RawValue;

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

/// Splits PKCS#1 `RSAPublicKey` DER into its unsigned modulus and exponent.
fn der_components(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let (body, rest) = der(bytes, 0x30)?;
    let (n, body) = der(body, 0x02)?;
    let (e, body) = der(body, 0x02)?;
    (rest.is_empty() && body.is_empty()).then_some((unsigned(n)?, unsigned(e)?))
}

fn trimmed(int: &[u8]) -> &[u8] {
    &int[int.iter().position(|&b| b != 0).unwrap_or(int.len())..]
}

/// Accepts RSA keys with a 2048..=8192-bit modulus and odd exponent >= 3, from PEM or JWK.
fn strong_rsa(key: &DecodingKey) -> Option<()> {
    let (n, e) = match key.kind() {
        DecodingKeyKind::SecretOrDer(bytes) => der_components(bytes)?,
        DecodingKeyKind::RsaModulusExponent { n, e } => (trimmed(n), trimmed(e)),
    };
    let bits = n.len() * 8 - n.first()?.leading_zeros() as usize;
    let odd = e.last()? & 1 == 1;
    ((2048..=8192).contains(&bits) && odd && (e.len() > 1 || e[0] >= 3)).then_some(())
}

/// A strong RS256 public key the crypto provider accepts.
pub(crate) fn usable(key: &DecodingKey) -> Option<()> {
    strong_rsa(key)?;
    (jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER.verifier_factory)(&Algorithm::RS256, key)
        .ok()
        .map(|_| ())
}

/// Object members in order, duplicates kept so they can be rejected.
struct Members(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Members {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visit;
        impl<'de> serde::de::Visitor<'de> for Visit {
            type Value = Members;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Members, A::Error> {
                let mut members = Vec::new();
                while let Some(member) = map.next_entry()? {
                    members.push(member);
                }
                Ok(Members(members))
            }
        }
        d.deserialize_map(Visit)
    }
}

/// Follows the dotted path; absent is no roles, a repeated key on the way is ambiguous.
fn roles(claims: &RawValue, path: &[String]) -> Result<Vec<String>, &'static str> {
    let mut current = claims.to_owned();
    for part in path {
        let Members(members) = serde_json::from_str(current.get()).map_err(|_| "invalid_claims")?;
        let mut found = members.into_iter().filter(|(key, _)| key == part);
        match (found.next(), found.next()) {
            (None, _) => return Ok(Vec::new()),
            (Some((_, value)), None) => current = value,
            _ => return Err("invalid_claims"),
        }
    }
    serde_json::from_str(current.get()).map_err(|_| "invalid_claims")
}

enum Keys {
    Local(DecodingKey),
    Jwks(Box<Jwks>),
}

pub struct Verifier {
    keys: Keys,
    validation: Validation,
    roles_claim: Vec<String>,
}

impl Verifier {
    fn with_keys(keys: Keys, issuer: &str, audience: &str) -> Result<Self, Error> {
        if issuer.trim().is_empty() || audience.trim().is_empty() {
            return Err(Error::InvalidRequest);
        }
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["sub", "iss", "aud", "exp"]);
        validation.set_issuer(&[issuer]);
        validation.set_audience(&[audience]);
        validation.validate_nbf = true;
        validation.leeway = DEFAULT_LEEWAY_SECONDS;
        validation.reject_tokens_expiring_in_less_than = 1;
        Ok(Self {
            keys,
            validation,
            roles_claim: vec!["roles".into()],
        })
    }

    pub fn new(pem: &[u8], issuer: &str, audience: &str) -> Result<Self, Error> {
        let pem = pem.trim_ascii();
        if !(pem.starts_with(b"-----BEGIN PUBLIC KEY-----")
            || pem.starts_with(b"-----BEGIN RSA PUBLIC KEY-----"))
        {
            return Err(Error::InvalidRequest);
        }
        let key = DecodingKey::from_rsa_pem(pem).map_err(|_| Error::InvalidRequest)?;
        usable(&key).ok_or(Error::InvalidRequest)?;
        Self::with_keys(Keys::Local(key), issuer, audience)
    }

    /// Trusts only the keys of the operator JWKS; `kid` selects within it.
    pub fn from_jwks(jwks: Jwks, issuer: &str, audience: &str) -> Result<Self, Error> {
        Self::with_keys(Keys::Jwks(Box::new(jwks)), issuer, audience)
    }

    /// Dotted path to the roles array, `roles` by default.
    pub fn with_roles_claim(mut self, path: &str) -> Result<Self, Error> {
        self.roles_claim = path.split('.').map(str::to_owned).collect();
        if self.roles_claim.iter().any(String::is_empty) {
            return Err(Error::InvalidRequest);
        }
        Ok(self)
    }

    /// Overrides the `exp`/`nbf` clock skew tolerance; range is enforced by configuration.
    pub fn with_leeway(mut self, seconds: u64) -> Self {
        self.validation.leeway = seconds;
        self
    }

    /// On failure returns a stable reason for logs; never the token or its claims.
    pub async fn verify(&self, token: &str) -> Result<Principal, &'static str> {
        // Bound decoding and signature work independently of future request body limits.
        if token.len() > 16384 {
            return Err("token_too_large");
        }
        let fetched;
        let key = match &self.keys {
            Keys::Local(key) => key,
            Keys::Jwks(jwks) => {
                let header = decode_header(token).map_err(|e| reason(e.kind()))?;
                fetched = jwks.key(&header.kid.ok_or("missing_kid")?).await?;
                &fetched
            }
        };
        let raw = decode::<Box<RawValue>>(token, key, &self.validation)
            .map_err(|e| reason(e.kind()))?
            .claims;
        let claims: Claims = serde_json::from_str(raw.get()).map_err(|_| "invalid_claims")?;
        let roles = roles(&raw, &self.roles_claim)?;
        if claims.sub.trim().is_empty() || roles.iter().any(|r| r.trim().is_empty()) {
            return Err("invalid_claims");
        }
        Ok(Principal {
            sub: Some(claims.sub),
            roles,
        })
    }
}
