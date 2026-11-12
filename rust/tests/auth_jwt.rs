use atlas::{auth::jwt::Verifier, config::Config};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, get_current_timestamp};
use serde_json::{Value, json};

const PRIVATE: &[u8] = include_bytes!("fixtures/jwt-test-private.pem");
const PUBLIC: &[u8] = include_bytes!("fixtures/jwt-test-public.pem");
const PUBLIC_1024: &[u8] = include_bytes!("fixtures/jwt-test-1024-public.pem");
const PUBLIC_4096: &[u8] = include_bytes!("fixtures/jwt-test-4096-public.pem");
// PKCS#1 form of the 2048-bit test key.
const PKCS1_2048: &str = "-----BEGIN RSA PUBLIC KEY-----
MIIBCgKCAQEAwQgotQ8ZtLVDIV3xlDw9XeCL3z6GTt8kGQdUA6Gv7tw5ilYS3uHh
B9/bCsXfAQPVYBsKqkg4OIGVaKtKa/k0Y8XYMahM02LZibB5kvh9uYM8U9HVjAS5
t9GZoCbMv5VG7pTSLNl7Q+hGH08DRBSmWqPYqkDjYrr3G11rIgNS9ZRwtr2DCOZO
wFSBq5cdCu39hyJpl5RrOnWh5sDSpTSnaECB27egIHaGmyq5zS41vobQfm8wc0gT
uMbJNCCCkgu/QY2HszO5l1JSG4TWnTW4Ma5m3VYnIETihtgm650XEQGF0mY5HkLS
gSLPlwfPOk1ActKnhrz4SX6VglbJ2a6sgQIDAQAB
-----END RSA PUBLIC KEY-----
";
// Same 2048-bit modulus with public exponents 1 and 2.
const EXPONENT_1: &str = "-----BEGIN RSA PUBLIC KEY-----
MIIBCAKCAQEAwQgotQ8ZtLVDIV3xlDw9XeCL3z6GTt8kGQdUA6Gv7tw5ilYS3uHh
B9/bCsXfAQPVYBsKqkg4OIGVaKtKa/k0Y8XYMahM02LZibB5kvh9uYM8U9HVjAS5
t9GZoCbMv5VG7pTSLNl7Q+hGH08DRBSmWqPYqkDjYrr3G11rIgNS9ZRwtr2DCOZO
wFSBq5cdCu39hyJpl5RrOnWh5sDSpTSnaECB27egIHaGmyq5zS41vobQfm8wc0gT
uMbJNCCCkgu/QY2HszO5l1JSG4TWnTW4Ma5m3VYnIETihtgm650XEQGF0mY5HkLS
gSLPlwfPOk1ActKnhrz4SX6VglbJ2a6sgQIBAQ==
-----END RSA PUBLIC KEY-----
";
const EXPONENT_2: &str = "-----BEGIN RSA PUBLIC KEY-----
MIIBCAKCAQEAwQgotQ8ZtLVDIV3xlDw9XeCL3z6GTt8kGQdUA6Gv7tw5ilYS3uHh
B9/bCsXfAQPVYBsKqkg4OIGVaKtKa/k0Y8XYMahM02LZibB5kvh9uYM8U9HVjAS5
t9GZoCbMv5VG7pTSLNl7Q+hGH08DRBSmWqPYqkDjYrr3G11rIgNS9ZRwtr2DCOZO
wFSBq5cdCu39hyJpl5RrOnWh5sDSpTSnaECB27egIHaGmyq5zS41vobQfm8wc0gT
uMbJNCCCkgu/QY2HszO5l1JSG4TWnTW4Ma5m3VYnIETihtgm650XEQGF0mY5HkLS
gSLPlwfPOk1ActKnhrz4SX6VglbJ2a6sgQIBAg==
-----END RSA PUBLIC KEY-----
";
// 256-byte modulus whose top bit is clear: 2047 bits, not 2048.
const MODULUS_2047: &str = "-----BEGIN RSA PUBLIC KEY-----
MIIBCQKCAQBimbsXtcHbJ9ffhLgDuR4QvmYeIIlYIX6oGX6HePhbWGGbN/XAmanq
m29JK/k0H438mD0kSSm5XwOgolPo2ZEIA2ThBuOhuwVjNU8aFrodw9NaQUnIzJ0M
Qdlr6xNcVEaCC954R4jzzA7jCy6m/SCM675ieKCDM3Ga8TlwqQ9cBIPQOcjKM2vI
ntARGs1uxeIQr3JQZCjn0lAAw+Xmk/o3ucFtkhaoNxnWgF/1OF+O4fkZncnwEc07
VmmaxnCmHKzbPNt6tiEUFEBS9vsRzczqo38hVEKfnqGPr9Pw2R4ObgkWWDUPQxWo
4ZGgpyZ6v5H7yIf1kWtgy0wA7gyVEkaXAgMBAAE=
-----END RSA PUBLIC KEY-----
";
// Odd 8200-bit modulus with exponent 65537, above the 8192-bit ceiling.
const MODULUS_8200: &str = "-----BEGIN RSA PUBLIC KEY-----
MIIECwKCBAIA2ObNezS1SaDr1nbsneIR4hupYhH5qGa8JYNug7RBAi8H0Cv+eztn
HAAbtMRV4LSkzZQZKoyJ7kaDgSq4BE0nV2gBl2zrwhwqg+Ya3AfsUtRIX5DtOoRa
0tBTL5XDtE4KZ/U2L9/WZCXZ9xV97PqM1ZdxsPLs6I8V9fsyaRvD5OPtXwBoQW7H
pApu+7rtnVXV/N/hnQDPbixIntods9hhYHYDffaij6L/Jsa7ujBFuXGjOeDdoKkw
KRjQdwokVRbw/SxKlXQwFrMVhnyjBQVV+KhIZW3bjzaIG7ye8qC54Y15I2GFYFNB
1HbKPvkGENURuepNLs9SQpcoLeBUElCD3mNphasKu5AKMiyygcMzZzZFg2yh7r+V
odmxDu7Kx6hyHqYbT8r59AZd7tQv74/X/s6G4OfVhBZ8uy1lfdwO5yy6PmA6B3D0
ffvGFVgqHPSeFQhpKZWOWK9iJ6JbzjGGhgs8telpspk0RNbEgjPkU09EcHsM/5u6
CKq4FeUH/qbi+X0ah0jZvg/pBDhevzXq9+c/rCD+x/TuwfQvJ8FCliAhbf1f048s
5yJEGu48hJxsMbhYEtQkhE6yfa3rdkemDl35ZatgWmhbXG0KqO+ZsUvFDb+P5YmP
y0unVgVMwLv3g4nf6t4e2QmxvSZ5ZXDOw0rdg0fF0tdm8b2Mg2NtfOIVOtEGgMP1
FJnXEllkWB5tiafgHXuAG2PLmKqePO/e+X33Sa7w+w/bQ7mzAfT7SdptSgsu0jOj
lOrKoTPpGXDe5qL2m2OYDLS7Qhp9qd0qytTDCRf9cilrLLxO8tjV4gXOJC8LalzU
kyp8rBu17LICoo4mSafZSEMYs1lKjNdchOVZI+o8qaF68zQIz0/fk+VJ0jLwme3r
z3Dy3a1VMVc1ulPcG436QH6P17ZUJfful5Eq7K9Oz/lR9K36D22SBsP8LREXys9Q
3cYHDSOjI7h/cDzU/sHwiYxpNy0CUxMXdv+V1U6Cuv/GZn5xXdOhZR2qd18Jpvbc
Mpl+bnY2/TZCE15w39MPkZM+823FzMeuUdpFd7iFyoMQchV6KdAmk7yh4Cb0IgGo
bpx2KkJbOPaROV3iwM7tyr3dN6qffC5V7AGTuJl8dj5/lTnQZkJHkVlHjXp4Cuk5
TcKLUvt1aPUrEG7MhtCdE3wfJQ2nC/UkYYo/Vdlt/iCp+mAFg4DgV8SZA5aCwxzg
1E/ME32L4Khp+Ij8eafgQDtJ7IBh316F2QhcKgdLUIj/0RL/i9sxebyQvnm4au8v
DwPA4aG6KmW+Q0iuUoz1qHzEPOMYn8W7xbhShF2KioRdVrQHhEDkzxHOCvPag/1N
zd1BDJptgPxxMTae9PzZVsU+sVLbIOrVb+ECAwEAAQ==
-----END RSA PUBLIC KEY-----
";

fn claims() -> Value {
    json!({"sub":"alice", "iss":"atlas-test", "aud":"atlas", "exp":get_current_timestamp()+3600,
        "nbf":get_current_timestamp()-60, "roles":["reader"]})
}

fn configured(leeway: Option<i64>) -> Verifier {
    let mut text = "[auth]\npublic_key_file='tests/fixtures/jwt-test-public.pem'\nissuer='atlas-test'\naudience='atlas'".to_string();
    if let Some(leeway) = leeway {
        text.push_str(&format!("\nleeway_seconds={leeway}"));
    }
    let config = Config::parse(&text, None, None).unwrap();
    config.auth.unwrap().verifier().unwrap()
}

fn token(claims: &Value) -> String {
    encode(
        &Header::new(Algorithm::RS256),
        claims,
        &EncodingKey::from_rsa_pem(PRIVATE).unwrap(),
    )
    .unwrap()
}

/// The rejection reason; the error type is the stable `&'static str` of the contract.
fn reason(verifier: &Verifier, token: &str) -> &'static str {
    verifier
        .verify(token)
        .err()
        .unwrap_or_else(|| panic!("accepted {token}"))
}

// R12
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

// R12
#[test]
fn invalid_claims_fail_closed() {
    let verifier = configured(Some(0));
    for field in ["sub", "iss", "aud", "exp"] {
        let mut c = claims();
        c.as_object_mut().unwrap().remove(field);
        assert_eq!(reason(&verifier, &token(&c)), "invalid_claims", "{field}");
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
        reason(&verifier, &token(&c));
    }
}

#[test]
fn each_rejection_reports_its_reason() {
    let verifier = Verifier::new(PUBLIC, "atlas-test", "atlas").unwrap();
    let now = get_current_timestamp();
    for (field, value, expected) in [
        ("exp", json!(now - 3600), "expired"),
        ("nbf", json!(now + 3600), "not_yet_valid"),
        ("iss", json!("other-issuer"), "invalid_issuer"),
        ("aud", json!("other"), "invalid_audience"),
    ] {
        let mut c = claims();
        c[field] = value;
        assert_eq!(reason(&verifier, &token(&c)), expected, "{c}");
    }
    let mut c = claims();
    c.as_object_mut().unwrap().remove("sub");
    assert_eq!(reason(&verifier, &token(&c)), "invalid_claims");

    let signed = token(&claims());
    let index = signed.rfind('.').unwrap() + 1;
    let mut broken = signed.into_bytes();
    broken[index] = if broken[index] == b'A' { b'B' } else { b'A' };
    let broken = String::from_utf8(broken).unwrap();
    assert_eq!(reason(&verifier, &broken), "invalid_signature");

    assert_eq!(reason(&verifier, &"x".repeat(20 * 1024)), "token_too_large");
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
        reason(&verifier, &invalid);
    }
    let mut header = Header::new(Algorithm::RS512);
    header.kid = Some("RS256".into());
    let wrong = encode(
        &header,
        &claims(),
        &EncodingKey::from_rsa_pem(PRIVATE).unwrap(),
    )
    .unwrap();
    assert_eq!(reason(&verifier, &wrong), "invalid_algorithm");
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
        reason(&verifier, &duplicate);
    }
}

#[test]
fn public_key_strength_is_checked_on_load() {
    for pem in [PUBLIC, PUBLIC_4096, PKCS1_2048.as_bytes()] {
        assert!(Verifier::new(pem, "atlas-test", "atlas").is_ok());
    }
    for (name, pem) in [
        ("1024", PUBLIC_1024),
        ("exponent 1", EXPONENT_1.as_bytes()),
        ("exponent 2", EXPONENT_2.as_bytes()),
        ("2047 bits", MODULUS_2047.as_bytes()),
        ("8200 bits", MODULUS_8200.as_bytes()),
    ] {
        assert!(Verifier::new(pem, "atlas-test", "atlas").is_err(), "{name}");
    }
    // Valid header and base64, truncated DER: drop whole 64-char lines (48 bytes each).
    let text = std::str::from_utf8(PUBLIC).unwrap();
    let lines: Vec<&str> = text.trim().lines().collect();
    let (header, body, footer) = (lines[0], &lines[1..lines.len() - 1], lines[lines.len() - 1]);
    for keep in 1..body.len() {
        let truncated = format!("{header}\n{}\n{footer}\n", body[..keep].join("\n"));
        assert!(
            Verifier::new(truncated.as_bytes(), "atlas-test", "atlas").is_err(),
            "{keep} lines"
        );
    }
}

#[test]
fn leeway_applies_to_exp_and_nbf() {
    let now = get_current_timestamp();
    let with = |field: &str, value: u64| {
        let mut c = claims();
        c[field] = json!(value);
        token(&c)
    };
    let default = configured(None);
    let strict = configured(Some(0));
    let wide = configured(Some(300));
    assert!(default.verify(&with("exp", now - 20)).is_ok());
    assert!(default.verify(&with("nbf", now + 20)).is_ok());
    assert!(default.verify(&with("exp", now - 60)).is_err());
    assert!(default.verify(&with("nbf", now + 60)).is_err());
    assert!(strict.verify(&with("exp", now - 20)).is_err());
    assert!(strict.verify(&with("nbf", now + 20)).is_err());
    assert!(strict.verify(&token(&claims())).is_ok());
    assert!(wide.verify(&with("exp", now - 280)).is_ok());
    assert!(wide.verify(&with("nbf", now + 280)).is_ok());
    assert!(wide.verify(&with("exp", now - 330)).is_err());
    assert!(wide.verify(&with("nbf", now + 330)).is_err());
}
