use super::respond;
use crate::{
    auth::policy::admin,
    domain::{Error, Principal},
    services::publications::{Publications, document},
};
use axum::{
    Extension, Router,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    response::Response,
    routing::get,
};
use serde_json::json;

#[derive(Clone)]
struct Admin {
    service: Publications,
}

pub fn router(service: Publications) -> Router {
    Router::new()
        .route("/api/v1/admin/datasets", get(list))
        .route("/api/v1/admin/datasets/{id}", get(read))
        .with_state(Admin { service })
}

async fn list(
    State(state): State<Admin>,
    Extension(principal): Extension<Principal>,
    RawQuery(query): RawQuery,
) -> Result<Response, Error> {
    admin(&principal)?;
    let (limit, offset) = paging(query.as_deref())?;
    let (page, more) = state.service.list(limit, offset).await?;
    let publications: Vec<_> = page.iter().map(document).collect();
    let body = json!({"publications": publications, "hasMore": more});
    Ok(respond(StatusCode::OK, Some(body), None))
}

async fn read(
    State(state): State<Admin>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    admin(&principal)?;
    let found = state.service.get(identifier(&id)?).await?;
    Ok(respond(
        StatusCode::OK,
        Some(document(&found)),
        Some(found.revision),
    ))
}

fn identifier(id: &str) -> Result<&str, Error> {
    let valid = !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-');
    valid.then_some(id).ok_or(Error::InvalidRequest)
}

fn paging(query: Option<&str>) -> Result<(Option<u32>, u32), Error> {
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
