use super::jwt::Verifier;
use crate::domain::{Error, Principal};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

fn principal(headers: &HeaderMap, verifier: Option<&Verifier>) -> Result<Principal, Error> {
    let mut values = headers.get_all("authorization").iter();
    let Some(value) = values.next() else {
        return Ok(Principal::default());
    };
    if values.next().is_some() {
        return Err(Error::InvalidToken);
    }
    let (scheme, token) = value
        .to_str()
        .ok()
        .and_then(|v| v.split_once(' '))
        .ok_or(Error::InvalidToken)?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.is_empty()
        || token.bytes().any(|b| b.is_ascii_whitespace())
    {
        return Err(Error::InvalidToken);
    }
    verifier.ok_or(Error::InvalidToken)?.verify(token)
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
        Err(_) => {
            static REQUEST_ID: AtomicU64 = AtomicU64::new(1);
            let body = serde_json::json!({"code": Error::InvalidToken,
                "message":"invalid token", "request_id":REQUEST_ID.fetch_add(1, Ordering::Relaxed).to_string()});
            (
                StatusCode::UNAUTHORIZED,
                [
                    ("content-type", "application/json"),
                    ("www-authenticate", "Bearer"),
                    ("cache-control", "no-store"),
                ],
                body.to_string(),
            )
                .into_response()
        }
    }
}
