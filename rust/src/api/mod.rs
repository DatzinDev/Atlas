pub mod admin;

use crate::domain::Error;
use axum::{
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

pub fn status(error: Error) -> StatusCode {
    match error {
        Error::InvalidRequest | Error::InvalidFilter => StatusCode::BAD_REQUEST,
        Error::InvalidToken => StatusCode::UNAUTHORIZED,
        Error::PermissionDenied => StatusCode::FORBIDDEN,
        Error::DatasetNotFound => StatusCode::NOT_FOUND,
        Error::PublicationConflict | Error::SchemaChanged => StatusCode::CONFLICT,
        Error::RevisionMismatch => StatusCode::PRECONDITION_FAILED,
        Error::QueryTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        Error::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
        Error::CapacityExceeded | Error::DatabaseUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        Error::QueryTimeout => StatusCode::GATEWAY_TIMEOUT,
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        failure(self)
    }
}

pub fn failure(error: Error) -> Response {
    static REQUEST_ID: AtomicU64 = AtomicU64::new(1);
    let code = serde_json::to_value(error).unwrap_or(Value::Null);
    let message = code.as_str().unwrap_or("error").replace('_', " ");
    let body = json!({"code": code, "message": message,
        "request_id": REQUEST_ID.fetch_add(1, Ordering::Relaxed).to_string()});
    respond(status(error), Some(body), None)
}

pub fn respond(status: StatusCode, body: Option<Value>, revision: Option<i64>) -> Response {
    let mut response = match body {
        Some(body) => (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            body.to_string(),
        )
            .into_response(),
        None => status.into_response(),
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(etag) = revision.and_then(|r| format!("\"{r}\"").parse().ok()) {
        headers.insert(header::ETAG, etag);
    }
    response
}
