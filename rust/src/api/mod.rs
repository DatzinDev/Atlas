pub mod admin;
pub mod datasets;
pub mod export;
pub mod features;
pub mod tiles;

use crate::domain::Error;
use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    sync::{
        LazyLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

tokio::task_local! {
    static REQUEST_ID: String;
}

/// `<nonce>-<n>`: a 16-hex nonce fixed at startup plus a process-wide counter.
fn next_id() -> String {
    static NONCE: LazyLock<String> = LazyLock::new(|| {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        format!(
            "{:016x}",
            nanos ^ u64::from(std::process::id()).rotate_left(32)
        )
    });
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!("{}-{}", *NONCE, COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// The current request's id; outside `track` (direct calls) a fresh one keeps output well-formed.
pub(crate) fn request_id() -> String {
    REQUEST_ID
        .try_with(String::clone)
        .unwrap_or_else(|_| next_id())
}

/// Writes one structured JSON line to stderr.
pub fn log(line: Value) {
    eprintln!("{line}");
}

/// Outermost middleware: assigns the request id, sets `x-request-id` and logs the request.
/// Client-sent `x-request-id` is ignored; never logs headers, bodies or the query string.
pub async fn track(request: Request, next: Next) -> Response {
    let (id, start) = (next_id(), Instant::now());
    let method = request.method().to_string();
    let mut path = request.uri().path().to_owned();
    let mut end = path.len().min(256);
    while !path.is_char_boundary(end) {
        end -= 1;
    }
    path.truncate(end);
    let mut response = REQUEST_ID.scope(id.clone(), next.run(request)).await;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert("x-request-id", value);
    }
    let mut line = json!({"event": "request", "request_id": id, "method": method,
        "path": path, "status": response.status().as_u16(),
        "duration_ms": start.elapsed().as_millis() as u64});
    if let Some(error) = response.extensions().get::<Error>() {
        line["code"] = json!(error);
    }
    log(line);
    response
}

pub fn status(error: Error) -> StatusCode {
    match error {
        Error::InvalidRequest | Error::InvalidFilter => StatusCode::BAD_REQUEST,
        Error::InvalidToken => StatusCode::UNAUTHORIZED,
        Error::PermissionDenied => StatusCode::FORBIDDEN,
        Error::DatasetNotFound | Error::NotFound => StatusCode::NOT_FOUND,
        Error::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
        Error::PublicationConflict | Error::SchemaChanged => StatusCode::CONFLICT,
        Error::RevisionMismatch => StatusCode::PRECONDITION_FAILED,
        Error::QueryTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        Error::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
        Error::CapacityExceeded | Error::DatabaseUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        Error::QueryTimeout => StatusCode::GATEWAY_TIMEOUT,
    }
}

/// Fixed English hint per code; never carries request data.
fn message(error: Error) -> &'static str {
    match error {
        Error::InvalidRequest => {
            "The request has an invalid or unsupported parameter, header or body."
        }
        Error::InvalidFilter => "The filter is not a valid predicate for this dataset.",
        Error::InvalidToken => "The bearer token is not valid.",
        Error::PermissionDenied => "The principal is not allowed to perform this operation.",
        Error::DatasetNotFound => "The dataset does not exist or is not visible.",
        Error::NotFound => "No route matches this path.",
        Error::MethodNotAllowed => "The method is not allowed for this route.",
        Error::PublicationConflict => "A publication with this id already exists.",
        Error::SchemaChanged => "The publication no longer matches its source.",
        Error::RevisionMismatch => "The If-Match revision does not match the current revision.",
        Error::QueryTooLarge => "The request exceeds a configured size or result budget.",
        Error::InternalError => "Unexpected server error; report the request_id.",
        Error::CapacityExceeded => "The server is at capacity; retry later.",
        Error::DatabaseUnavailable => "The database is unavailable; retry later.",
        Error::QueryTimeout => "The query exceeded its time budget.",
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        failure(self)
    }
}

pub fn failure(error: Error) -> Response {
    let code = serde_json::to_value(error).unwrap_or(Value::Null);
    let body = json!({"code": code, "message": message(error), "request_id": request_id()});
    let mut response = respond(status(error), Some(body), None);
    response.extensions_mut().insert(error);
    response
}

pub async fn nosniff(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
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

pub(crate) fn no_query(query: Option<&str>) -> Result<(), Error> {
    match query {
        Some(query) if !query.is_empty() => Err(Error::InvalidRequest),
        _ => Ok(()),
    }
}

// Exactly one Content-Type: application/json (or merge-patch+json when allowed), charset utf-8 only.
pub(crate) fn json_body(headers: &HeaderMap, merge_patch: bool) -> Result<(), Error> {
    let mut values = headers.get_all(header::CONTENT_TYPE).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return Err(Error::InvalidRequest);
    };
    let value = value
        .to_str()
        .map_err(|_| Error::InvalidRequest)?
        .to_ascii_lowercase();
    let mut parts = value.split(';').map(str::trim);
    let media = parts.next().unwrap_or_default();
    let valid = (media == "application/json"
        || (merge_patch && media == "application/merge-patch+json"))
        && parts.all(|p| p == "charset=utf-8");
    valid.then_some(()).ok_or(Error::InvalidRequest)
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
                if !hex.iter().all(u8::is_ascii_hexdigit) {
                    return Err(Error::InvalidRequest);
                }
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
