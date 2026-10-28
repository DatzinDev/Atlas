use atlas::{auth::jwt::Verifier, domain::Error};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, get_current_timestamp};
use serde_json::{Value, json};

const PRIVATE: &[u8] = include_bytes!("fixtures/jwt-test-private.pem");
const PUBLIC: &[u8] = include_bytes!("fixtures/jwt-test-public.pem");

fn claims() -> Value {
    json!({"sub":"alice", "iss":"atlas-test", "aud":"atlas", "exp":get_current_timestamp()+3600,
        "nbf":get_current_timestamp()-60, "roles":["reader"]})
}

fn token(claims: &Value) -> String {
    encode(
        &Header::new(Algorithm::RS256),
        claims,
        &EncodingKey::from_rsa_pem(PRIVATE).unwrap(),
    )
    .unwrap()
}

#[test]
fn valid_identity_and_local_trust_only() {
    let verifier = Verifier::new(PUBLIC, "atlas-test", "atlas").unwrap();
    let principal = verifier.verify(&token(&claims())).unwrap();
    assert_eq!(principal.sub.as_deref(), Some("alice"));
    assert_eq!(principal.roles, ["reader"]);
    let mut c = claims();
    c["aud"] = json!(["other", "atlas"]);
    c.as_object_mut().unwrap().remove("roles");
    c.as_object_mut().unwrap().remove("nbf");
    c["external_claim"] = json!(true);
    assert!(verifier.verify(&token(&c)).unwrap().roles.is_empty());
    let mut header = Header::new(Algorithm::RS256);
    header.jku = Some("https://untrusted.invalid/jwks".into());
    header.kid = Some("ignored-key".into());
    let signed = encode(
        &header,
        &claims(),
        &EncodingKey::from_rsa_pem(PRIVATE).unwrap(),
    )
    .unwrap();
    assert!(verifier.verify(&signed).is_ok());
    for (pem, issuer, audience) in [
        (b"invalid".as_slice(), "issuer", "aud"),
        (PRIVATE, "issuer", "aud"),
        (PUBLIC, "", "aud"),
        (PUBLIC, "issuer", " "),
    ] {
        assert!(Verifier::new(pem, issuer, audience).is_err());
    }
}

#[test]
fn invalid_claims_fail_closed() {
    let verifier = Verifier::new(PUBLIC, "atlas-test", "atlas").unwrap();
    for field in ["sub", "iss", "aud", "exp"] {
        let mut c = claims();
        c.as_object_mut().unwrap().remove(field);
        assert!(matches!(
            verifier.verify(&token(&c)),
            Err(Error::InvalidToken)
        ));
    }
    for (field, value) in [
        ("sub", json!(" ")),
        ("sub", json!(1)),
        ("iss", json!("wrong")),
        ("iss", json!(null)),
        ("aud", json!("wrong")),
        ("aud", json!([])),
        ("aud", json!(["atlas", 1])),
        ("exp", json!(get_current_timestamp() - 1)),
        ("exp", json!(get_current_timestamp())),
        ("exp", json!("9999999999")),
        ("exp", json!(1.5)),
        ("exp", json!(-1)),
        ("exp", json!(null)),
        ("nbf", json!(get_current_timestamp() + 3600)),
        ("nbf", json!(null)),
        ("nbf", json!("1")),
        ("nbf", json!(-1)),
        ("roles", json!("reader")),
        ("roles", json!([1])),
        ("roles", json!([""])),
        ("roles", json!(null)),
    ] {
        let mut c = claims();
        c[field] = value;
        assert!(
            matches!(verifier.verify(&token(&c)), Err(Error::InvalidToken)),
            "{field}: {c}"
        );
    }
}

#[test]
fn signatures_algorithms_and_malformed_input() {
    let verifier = Verifier::new(PUBLIC, "atlas-test", "atlas").unwrap();
    let signed = token(&claims());
    let mut broken = signed.clone().into_bytes();
    let index = signed.rfind('.').unwrap() + 1;
    broken[index] = if broken[index] == b'A' { b'B' } else { b'A' };
    let hs = encode(
        &Header::new(Algorithm::HS256),
        &claims(),
        &EncodingKey::from_secret(PUBLIC),
    )
    .unwrap();
    for invalid in [
        "".into(),
        "a.b.c".into(),
        "eyJhbGciOiJub25lIn0.e30.".into(),
        hs,
        String::from_utf8(broken).unwrap(),
        "x".repeat(16385),
    ] {
        assert!(matches!(
            verifier.verify(&invalid),
            Err(Error::InvalidToken)
        ));
    }
    let mut header = Header::new(Algorithm::RS512);
    header.kid = Some("RS256".into());
    let wrong = encode(
        &header,
        &claims(),
        &EncodingKey::from_rsa_pem(PRIVATE).unwrap(),
    )
    .unwrap();
    assert!(verifier.verify(&wrong).is_err());
    for field in ["sub", "iss", "aud", "exp", "nbf", "roles"] {
        let c = claims();
        let raw = format!("{{\"{field}\":{},{}", c[field], &c.to_string()[1..]);
        let raw = serde_json::value::RawValue::from_string(raw).unwrap();
        let duplicate = encode(
            &Header::new(Algorithm::RS256),
            &raw,
            &EncodingKey::from_rsa_pem(PRIVATE).unwrap(),
        )
        .unwrap();
        assert!(verifier.verify(&duplicate).is_err(), "duplicate {field}");
    }
}
