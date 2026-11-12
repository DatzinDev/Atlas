use super::jwt::Verifier;
use crate::api::{failure, log, request_id};
use crate::domain::{Error, Principal};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::Response,
};
use serde_json::json;
use std::sync::Arc;

/// Resolves the request principal; on failure returns the stable reason logged by `authenticate`.
pub fn principal(
    headers: &HeaderMap,
    verifier: Option<&Verifier>,
) -> Result<Principal, &'static str> {
    let mut values = headers.get_all("authorization").iter();
    let Some(value) = values.next() else {
        return Ok(Principal::default());
    };
    if values.next().is_some() {
        return Err("multiple_headers");
    }
    let (scheme, token) = value
        .to_str()
        .ok()
        .and_then(|v| v.split_once(' '))
        .ok_or("malformed_header")?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return Err("unsupported_scheme");
    }
    if token.is_empty() || token.bytes().any(|b| b.is_ascii_whitespace()) {
        return Err("malformed_header");
    }
    verifier.ok_or("auth_disabled")?.verify(token)
}

pub async fn authenticate(
    State(verifier): State<Arc<Option<Verifier>>>,
    mut request: Request,
    next: Next,
) -> Response {
    match principal(request.headers(), verifier.as_ref().as_ref()) {
        Ok(principal) => {
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(reason) => {
            log(json!({"event": "auth_failed", "request_id": request_id(), "reason": reason}));
            let mut response = failure(Error::InvalidToken);
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            response
        }
    }
}
