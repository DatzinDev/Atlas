use std::{env, process::ExitCode};

use atlas::{
    api::{failure, nosniff, track},
    config::Config,
    domain::Error,
};
use axum::{
    Router,
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use tokio::{net::TcpListener, signal};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// `None` when no origin is configured: no CORS headers and OPTIONS stays 405.
fn cors(origins: &[String]) -> Option<CorsLayer> {
    if origins.is_empty() {
        return None;
    }
    let origin = if origins == ["*"] {
        AllowOrigin::any()
    } else {
        // Config validation guarantees ASCII origins, so no entry is dropped.
        AllowOrigin::list(origins.iter().filter_map(|o| HeaderValue::from_str(o).ok()))
    };
    Some(
        CorsLayer::new()
            .allow_origin(origin)
            .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
            .allow_headers([
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                header::IF_MATCH,
            ])
            .expose_headers([
                header::ETAG,
                header::CONTENT_DISPOSITION,
                HeaderName::from_static("x-request-id"),
            ])
            .max_age(std::time::Duration::from_secs(600)),
    )
}

fn configuration() -> Result<(bool, Config), &'static str> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args
        .first()
        .is_none_or(|command| command != "serve" && command != "migrate")
        || args.len() % 2 == 0
    {
        return Err("usage: atlas serve|migrate [--config FILE] [--listen IP:PORT]");
    }
    let (mut path, mut listen) = (None, None);
    for pair in args[1..].as_chunks::<2>().0 {
        match pair[0].to_str() {
            Some("--config") if path.is_none() => path = Some(std::path::Path::new(&pair[1])),
            Some("--listen") if listen.is_none() => {
                listen = Some(pair[1].to_str().ok_or("invalid configuration")?.to_owned())
            }
            _ => return Err("invalid configuration"),
        }
    }
    Config::load(path, listen)
        .map(|config| (args[0] == "migrate", config))
        .map_err(|_| "invalid configuration")
}

async fn serve(config: Config) -> Result<(), &'static str> {
    if config.database_url.is_none() {
        return Err("database url required");
    }
    let verifier = std::sync::Arc::new(
        config
            .auth
            .as_ref()
            .map(|auth| auth.verifier())
            .transpose()
            .map_err(|_| "invalid authentication configuration")?,
    );
    // Register before accepting connections so an early signal cannot bypass draining.
    #[cfg(unix)]
    let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())
        .map_err(|_| "cannot register shutdown signal")?;
    #[cfg(unix)]
    let mut interrupt = signal::unix::signal(signal::unix::SignalKind::interrupt())
        .map_err(|_| "cannot register shutdown signal")?;
    #[cfg(not(unix))]
    let interrupt = signal::ctrl_c();
    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|_| "cannot bind listener")?;
    let bound = listener
        .local_addr()
        .map_err(|_| "cannot inspect listener")?;
    let database = atlas::postgis::connection::Database::open(&config)
        .await
        .map_err(|_| "database unavailable")?;
    let catalog = atlas::catalog::Catalog::new(database.clone(), config.clone())
        .map_err(|_| "invalid configuration")?;
    // Probes stay outside authentication: OpenAPI declares them `security: []`.
    let probes = Router::new()
        .route("/api/v1/health", get(|| async { StatusCode::OK }))
        .route(
            "/api/v1/ready",
            get(move || {
                let catalog = catalog.clone();
                async move {
                    match catalog.ready().await {
                        Ok(()) => StatusCode::OK.into_response(),
                        Err(e @ Error::CapacityExceeded) => failure(e),
                        Err(_) => failure(Error::DatabaseUnavailable),
                    }
                }
            }),
        );
    let service =
        atlas::services::publications::Publications::new(database.clone(), config.clone())
            .map_err(|_| "invalid configuration")?;
    let router = Router::new()
        .merge(atlas::api::datasets::router(service.clone()))
        .merge(atlas::api::tiles::router(service.clone()))
        .merge(atlas::api::features::router(service.clone()))
        .merge(atlas::api::export::router(service.clone()))
        .merge(atlas::api::admin::router(
            service,
            config.limits.admin_body_bytes,
        ));
    let router = router
        .layer(axum::middleware::from_fn_with_state(
            verifier,
            atlas::auth::http::authenticate,
        ))
        .merge(probes)
        .fallback(|| async { failure(Error::NotFound) })
        // Set after every route is added: axum applies it to existing routes and keeps `Allow`.
        .method_not_allowed_fallback(|| async { failure(Error::MethodNotAllowed) })
        .layer(axum::middleware::map_response(nosniff))
        // Every routed response, including fallbacks and 401s, gets the request id.
        .layer(axum::middleware::from_fn(track));
    // Outside `track` and auth: preflights need no token and every error carries CORS.
    let router = match cors(&config.cors.allowed_origins) {
        Some(layer) => router.layer(layer),
        None => router,
    };
    if config.public_url.is_none() {
        eprintln!("{{\"event\":\"public_url_missing\"}}");
    }
    eprintln!("{{\"event\":\"listening\",\"address\":\"{bound}\"}}");
    let (started, signalled) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            tokio::select! {
                _ = interrupt.recv() => {},
                _ = terminate.recv() => {},
            }
            #[cfg(not(unix))]
            let _ = interrupt.await;
            eprintln!("{{\"event\":\"shutdown_started\"}}");
            let _ = started.send(());
        })
        .into_future();
    let drain = std::time::Duration::from_secs(config.limits.shutdown_seconds.into());
    // Dropping the server stops tracking leftover connections; their tasks (and any export
    // transaction, rolled back when its connection drops) are aborted when the runtime exits.
    let result = tokio::select! {
        result = server => result,
        () = async {
            let _ = signalled.await;
            tokio::time::sleep(drain).await;
        } => {
            eprintln!("{{\"event\":\"shutdown_forced\"}}");
            Ok(())
        }
    };
    // Connections still held by aborted work keep `close` waiting; bound it.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), database.close()).await;
    result.map_err(|_| "server failed")?;
    eprintln!("{{\"event\":\"shutdown_complete\"}}");
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let result = match configuration() {
        Ok((false, config)) => serve(config).await,
        Ok((true, config)) => atlas::migrations::migrate(&config)
            .await
            .map_err(|_| "migration failed"),
        Err(message) => Err(message),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{{\"event\":\"startup_failed\",\"message\":\"{message}\"}}");
            ExitCode::FAILURE
        }
    }
}
