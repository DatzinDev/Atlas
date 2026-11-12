use super::{identifier, json_body, no_query, number, params};
use crate::{
    domain::{Direction, Error, Principal, Query, SortField},
    services::publications::Publications,
};
use axum::{
    Extension, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, RawQuery, State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::value::RawValue;

pub fn router(service: Publications) -> Router {
    let limit = service.limits().query_body_bytes as usize;
    Router::new()
        .route("/api/v1/datasets/{id}/items", get(items))
        .route("/api/v1/datasets/{id}/query", post(query))
        .layer(DefaultBodyLimit::max(limit))
        .with_state(service)
}

async fn items(
    State(service): State<Publications>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, Error> {
    let id = identifier(&id)?;
    let selection = parse(raw.as_deref())?;
    select(&service, &principal, id, &selection).await
}

async fn query(
    State(service): State<Publications>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Error> {
    let id = identifier(&id)?;
    no_query(raw.as_deref())?;
    json_body(&headers, false)?;
    let body = body.map_err(|rejection| match rejection.status() {
        StatusCode::PAYLOAD_TOO_LARGE => Error::QueryTooLarge,
        _ => Error::InvalidRequest,
    })?;
    let selection: Query = serde_json::from_slice(&body).map_err(|_| Error::InvalidRequest)?;
    select(&service, &principal, id, &selection).await
}

async fn select(
    service: &Publications,
    principal: &Principal,
    id: &str,
    selection: &Query,
) -> Result<Response, Error> {
    let limits = service.limits();
    selection.validate(limits.max_limit, limits.max_offset)?;
    if let Some(filter) = &selection.filter {
        crate::query::predicate::parse(filter.get(), limits)?;
    }
    let publication = service.detail(id, principal).await?;
    let page = service.features(&publication, selection).await?;
    let headers = [
        (header::CONTENT_TYPE, "application/geo+json"),
        (header::CACHE_CONTROL, "no-store"),
    ];
    Ok((headers, page.body).into_response())
}

pub(crate) fn parse(raw: Option<&str>) -> Result<Query, Error> {
    let [filter, bbox, properties, order, limit, offset] = params(
        raw,
        ["filter", "bbox", "properties", "sort", "limit", "offset"],
    )?;
    let mut selection = selection(filter, bbox, properties)?;
    selection.sort = order
        .map(|v| v.split(',').map(sort).collect::<Result<Vec<_>, _>>())
        .transpose()?;
    selection.limit = limit.as_deref().map(number).transpose()?;
    selection.offset = offset.as_deref().map(number).transpose()?.unwrap_or(0);
    Ok(selection)
}

/// The `filter`/`bbox`/`properties` part of a GET Query, shared by items and export.
pub(crate) fn selection(
    filter: Option<String>,
    bbox: Option<String>,
    properties: Option<String>,
) -> Result<Query, Error> {
    let filter = filter.map(RawValue::from_string).transpose();
    let bbox: Option<[f64; 4]> = bbox
        .map(|v| {
            let numbers: Vec<f64> = v
                .split(',')
                .map(|n| n.parse().map_err(|_| Error::InvalidRequest))
                .collect::<Result<_, _>>()?;
            numbers.try_into().map_err(|_| Error::InvalidRequest)
        })
        .transpose()?;
    Ok(Query {
        filter: filter.map_err(|_| Error::InvalidFilter)?,
        bbox,
        properties: properties.map(|v| v.split(',').map(str::to_owned).collect()),
        ..Query::default()
    })
}

fn sort(text: &str) -> Result<SortField, Error> {
    let (field, direction) = text.rsplit_once(':').ok_or(Error::InvalidRequest)?;
    let direction = match direction {
        "asc" => Direction::Asc,
        "desc" => Direction::Desc,
        _ => return Err(Error::InvalidRequest),
    };
    Ok(SortField {
        field: field.to_owned(),
        direction,
    })
}
