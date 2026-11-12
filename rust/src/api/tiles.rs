use super::{identifier, params, respond};
use crate::{
    domain::{Error, Principal},
    postgis::mvt::bounds,
    services::publications::Publications,
};
use axum::{
    Extension, Router,
    extract::{Path, RawQuery, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};

pub fn router(service: Publications) -> Router {
    Router::new()
        .route("/api/v1/datasets/{id}/tiles/{z}/{x}/{tile}", get(vector))
        .route("/api/v1/datasets/{id}/tilejson.json", get(tilejson))
        .with_state(service)
}

async fn vector(
    State(service): State<Publications>,
    Extension(principal): Extension<Principal>,
    Path((id, z, x, tile)): Path<(String, String, String, String)>,
    RawQuery(query): RawQuery,
) -> Result<Response, Error> {
    let id = identifier(&id)?;
    let y = tile.strip_suffix(".mvt").ok_or(Error::InvalidRequest)?;
    let xyz = (number(&z)?, number(&x)?, number(y)?);
    bounds(xyz.0, xyz.1, xyz.2, service.limits().tile_zoom)?;
    let (filter, properties) = parameters(query.as_deref())?;
    if let Some(filter) = &filter {
        crate::query::predicate::parse(filter, service.limits())?;
    }
    let publication = service.detail(id, &principal).await?;
    let body = service
        .tile(&publication, xyz, filter.as_deref(), properties.as_deref())
        .await?;
    let headers = [
        (header::CONTENT_TYPE, "application/vnd.mapbox-vector-tile"),
        (header::CACHE_CONTROL, "no-store"),
    ];
    Ok((headers, body).into_response())
}

async fn tilejson(
    State(service): State<Publications>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
) -> Result<Response, Error> {
    let id = identifier(&id)?;
    let (filter, properties) = parameters(query.as_deref())?;
    if let Some(filter) = &filter {
        crate::query::predicate::parse(filter, service.limits())?;
    }
    let publication = service.detail(id, &principal).await?;
    let body = service
        .tilejson(&publication, filter.as_deref(), properties.as_deref())
        .await?;
    Ok(respond(StatusCode::OK, Some(body), None))
}

fn number(text: &str) -> Result<u32, Error> {
    if text.is_empty() || text.len() > 10 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::InvalidRequest);
    }
    text.parse().map_err(|_| Error::InvalidRequest)
}

type Parameters = (Option<String>, Option<Vec<String>>);

fn parameters(query: Option<&str>) -> Result<Parameters, Error> {
    let [filter, properties] = params(query, ["filter", "properties"])?;
    let properties: Option<Vec<String>> =
        properties.map(|v| v.split(',').map(str::to_owned).collect());
    if properties.iter().flatten().any(String::is_empty) {
        return Err(Error::InvalidRequest);
    }
    Ok((filter, properties))
}
