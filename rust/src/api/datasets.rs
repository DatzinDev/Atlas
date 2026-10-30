use super::{identifier, paging, respond};
use crate::{
    domain::{Error, Principal},
    services::publications::{Publications, summary},
};
use axum::{
    Extension, Router,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    response::Response,
    routing::get,
};
use serde_json::json;

pub fn router(service: Publications) -> Router {
    Router::new()
        .route("/api/v1/datasets", get(list))
        .route("/api/v1/datasets/{id}", get(read))
        .with_state(service)
}

async fn list(
    State(service): State<Publications>,
    Extension(principal): Extension<Principal>,
    RawQuery(query): RawQuery,
) -> Result<Response, Error> {
    let (limit, offset) = paging(query.as_deref())?;
    let (page, more) = service.visible(&principal, limit, offset).await?;
    let datasets: Vec<_> = page.iter().map(summary).collect();
    let body = json!({"datasets": datasets, "hasMore": more});
    Ok(respond(StatusCode::OK, Some(body), None))
}

async fn read(
    State(service): State<Publications>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let found = service.detail(identifier(&id)?, &principal).await?;
    Ok(respond(StatusCode::OK, Some(summary(&found)), None))
}
