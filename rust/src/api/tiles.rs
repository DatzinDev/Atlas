use super::{identifier, respond};
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
    if query.is_some_and(|q| !q.is_empty()) {
        return Err(Error::InvalidRequest);
    }
    let publication = service.detail(identifier(&id)?, &principal).await?;
    Ok(respond(
        StatusCode::OK,
        Some(service.tilejson(&publication)),
        None,
    ))
}

fn number(text: &str) -> Result<u32, Error> {
    if text.is_empty() || text.len() > 10 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::InvalidRequest);
    }
    text.parse().map_err(|_| Error::InvalidRequest)
}

type Parameters = (Option<String>, Option<Vec<String>>);

fn parameters(query: Option<&str>) -> Result<Parameters, Error> {
    let (mut filter, mut properties) = (None, None);
    for pair in query
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
    {
        let (key, value) = pair.split_once('=').ok_or(Error::InvalidRequest)?;
        let value = decode(value)?;
        match key {
            "filter" if filter.is_none() => filter = Some(value),
            "properties" if properties.is_none() => {
                let names: Vec<String> = value.split(',').map(str::to_owned).collect();
                if names.iter().any(String::is_empty) {
                    return Err(Error::InvalidRequest);
                }
                properties = Some(names);
            }
            _ => return Err(Error::InvalidRequest),
        }
    }
    Ok((filter, properties))
}

fn decode(text: &str) -> Result<String, Error> {
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
