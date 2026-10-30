use super::{identifier, paging, respond};
use crate::{
    auth::policy::admin,
    domain::{Error, Principal},
    services::publications::{Publications, document},
};
use axum::{
    Extension, Router,
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::get,
};
use serde_json::json;

#[derive(Clone)]
struct Admin {
    service: Publications,
    body_limit: usize,
}

pub fn router(service: Publications, body_limit: u32) -> Router {
    Router::new()
        .route("/api/v1/admin/datasets", get(list).post(create))
        .route(
            "/api/v1/admin/datasets/{id}",
            get(read).patch(update).delete(remove),
        )
        .with_state(Admin {
            service,
            body_limit: body_limit as usize,
        })
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

async fn create(
    State(state): State<Admin>,
    Extension(principal): Extension<Principal>,
    body: Bytes,
) -> Result<Response, Error> {
    admin(&principal)?;
    if body.len() > state.body_limit {
        return Err(Error::QueryTooLarge);
    }
    let created = state.service.create(&body).await?;
    Ok(respond(
        StatusCode::CREATED,
        Some(document(&created)),
        Some(created.revision),
    ))
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

async fn update(
    State(state): State<Admin>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Error> {
    admin(&principal)?;
    if body.len() > state.body_limit {
        return Err(Error::QueryTooLarge);
    }
    let revision = if_match(&headers)?;
    let updated = state
        .service
        .update(identifier(&id)?, &body, revision)
        .await?;
    Ok(respond(
        StatusCode::OK,
        Some(document(&updated)),
        Some(updated.revision),
    ))
}

async fn remove(
    State(state): State<Admin>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    admin(&principal)?;
    let revision = if_match(&headers)?;
    state.service.delete(identifier(&id)?, revision).await?;
    Ok(respond(StatusCode::NO_CONTENT, None, None))
}

fn if_match(headers: &HeaderMap) -> Result<i64, Error> {
    let mut values = headers.get_all(header::IF_MATCH).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return Err(Error::InvalidRequest);
    };
    value
        .to_str()
        .ok()
        .and_then(|v| v.strip_prefix('"')?.strip_suffix('"'))
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|r| *r > 0)
        .ok_or(Error::InvalidRequest)
}
