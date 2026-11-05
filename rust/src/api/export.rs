use super::{decode, features::parse, identifier};
use crate::{
    domain::{Error, Principal},
    services::publications::Publications,
};
use axum::{
    Extension, Router,
    body::Body,
    extract::{Path, RawQuery, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};

pub fn router(service: Publications) -> Router {
    Router::new()
        .route("/api/v1/datasets/{id}/export", get(export))
        .with_state(service)
}

async fn export(
    State(service): State<Publications>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, Error> {
    let id = identifier(&id)?;
    let mut selection = Vec::new();
    let mut format_seen = false;
    for pair in raw
        .as_deref()
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
    {
        let (key, value) = pair.split_once('=').ok_or(Error::InvalidRequest)?;
        match key {
            "format" if !format_seen && decode(value)? == "geojson" => format_seen = true,
            "filter" | "bbox" | "properties" => selection.push(pair),
            _ => return Err(Error::InvalidRequest),
        }
    }
    let query = parse(Some(&selection.join("&")))?;
    let export = service.export(id, &principal, &query).await?;
    let stream = futures_util::stream::try_unfold(export, |mut export| async move {
        match export.next().await {
            Ok(Some(chunk)) => Ok(Some((chunk, export))),
            Ok(None) => Ok(None),
            Err(error) => {
                eprintln!(
                    "{}",
                    serde_json::json!({"event":"export_failed", "code":error})
                );
                Err(std::io::Error::other("export interrupted"))
            }
        }
    });
    Ok((
        [
            (header::CONTENT_TYPE, "application/geo+json".to_owned()),
            (header::CACHE_CONTROL, "no-store".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{id}.geojson\""),
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}
