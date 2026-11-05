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
    let mut export = service.export(id, &principal, &query).await?;
    let deadline = export.deadline();
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    tokio::spawn(async move {
        loop {
            let chunk = export.next().await;
            let finished = !matches!(chunk, Ok(Some(_)));
            if let Err(error) = &chunk {
                eprintln!(
                    "{}",
                    serde_json::json!({"event":"export_failed", "code":error})
                );
            }
            match tokio::time::timeout_at(deadline, sender.send(chunk)).await {
                Ok(Ok(())) if !finished => {}
                Err(_) => {
                    eprintln!(
                        "{}",
                        serde_json::json!({"event":"export_failed", "code":"query_timeout"})
                    );
                    break;
                }
                _ => break,
            }
        }
    });
    let stream = futures_util::stream::try_unfold(receiver, |mut receiver| async move {
        match receiver.recv().await.unwrap_or(Err(Error::QueryTimeout))? {
            Some(chunk) => Ok::<_, Error>(Some((chunk, receiver))),
            None => Ok(None),
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
