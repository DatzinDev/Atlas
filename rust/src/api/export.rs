use super::{
    features::{checked, selection},
    identifier, params,
};
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
    let [format, filter, bbox, properties] =
        params(raw.as_deref(), ["format", "filter", "bbox", "properties"])?;
    if format.is_some_and(|f| f != "geojson") {
        return Err(Error::InvalidRequest);
    }
    let query = selection(filter, bbox, properties)?;
    let filter = checked(&query, service.limits())?;
    let mut export = service
        .export(id, &principal, &query, filter.as_ref())
        .await?;
    let deadline = export.deadline();
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    // Task-locals do not cross `spawn`: capture the id of the request that opened the download.
    let request_id = super::request_id();
    tokio::spawn(async move {
        loop {
            let chunk = export.next().await;
            let finished = !matches!(chunk, Ok(Some(_)));
            let mut failed = chunk.as_ref().err().copied();
            let sent = tokio::time::timeout_at(deadline, sender.send(chunk)).await;
            if sent.is_err() {
                failed = failed.or(Some(Error::QueryTimeout));
            }
            // One line per failed stream: the chunk error wins over a later send timeout.
            if let Some(code) = failed {
                super::log(serde_json::json!({"event": "export_failed",
                    "request_id": request_id, "code": code}));
            }
            if finished || !matches!(sent, Ok(Ok(()))) {
                break;
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
