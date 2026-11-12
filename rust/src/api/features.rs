use super::{decode, identifier, no_query};
use crate::{
    domain::{Direction, Error, Principal, Query, SortField},
    services::publications::Publications,
};
use axum::{
    Extension, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, RawQuery, State, rejection::BytesRejection},
    http::{StatusCode, header},
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
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Error> {
    let id = identifier(&id)?;
    no_query(raw.as_deref())?;
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
    let mut selection = Query::default();
    let mut seen = Vec::new();
    for pair in raw.unwrap_or_default().split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').ok_or(Error::InvalidRequest)?;
        if seen.contains(&key) {
            return Err(Error::InvalidRequest);
        }
        seen.push(key);
        let value = decode(value)?;
        let list = || value.split(',').map(str::to_owned).collect::<Vec<_>>();
        match key {
            "filter" => {
                let raw = RawValue::from_string(value.clone()).map_err(|_| Error::InvalidFilter)?;
                selection.filter = Some(raw);
            }
            "bbox" => {
                let numbers: Vec<f64> = list()
                    .iter()
                    .map(|v| v.parse().map_err(|_| Error::InvalidRequest))
                    .collect::<Result<_, _>>()?;
                selection.bbox = Some(numbers.try_into().map_err(|_| Error::InvalidRequest)?);
            }
            "properties" => selection.properties = Some(list()),
            "sort" => {
                selection.sort = Some(list().iter().map(|v| sort(v)).collect::<Result<_, _>>()?)
            }
            "limit" => selection.limit = Some(number(&value)?),
            "offset" => selection.offset = number(&value)?,
            _ => return Err(Error::InvalidRequest),
        }
    }
    Ok(selection)
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

fn number(text: &str) -> Result<u32, Error> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::InvalidRequest);
    }
    text.parse().map_err(|_| Error::InvalidRequest)
}
