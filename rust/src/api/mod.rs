pub mod admin;
pub mod datasets;
pub mod tiles;

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

pub(crate) fn identifier(id: &str) -> Result<&str, Error> {
    let valid = !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-');
    valid.then_some(id).ok_or(Error::InvalidRequest)
}

pub(crate) fn paging(query: Option<&str>) -> Result<(Option<u32>, u32), Error> {
    let (mut limit, mut offset) = (None, None);
    for pair in query
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
    {
        let (key, value) = pair.split_once('=').ok_or(Error::InvalidRequest)?;
        let slot = match key {
            "limit" if limit.is_none() => &mut limit,
            "offset" if offset.is_none() => &mut offset,
            _ => return Err(Error::InvalidRequest),
        };
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::InvalidRequest);
        }
        *slot = Some(value.parse::<u32>().map_err(|_| Error::InvalidRequest)?);
    }
    Ok((limit, offset.unwrap_or(0)))
}

pub(crate) fn decode(text: &str) -> Result<String, Error> {
    let (bytes, mut out, mut i) = (text.as_bytes(), Vec::new(), 0);
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3).ok_or(Error::InvalidRequest)?;
                let hex = std::str::from_utf8(hex).map_err(|_| Error::InvalidRequest)?;
                out.push(u8::from_str_radix(hex, 16).map_err(|_| Error::InvalidRequest)?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| Error::InvalidRequest)
}
