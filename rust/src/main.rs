use std::{env, process::ExitCode};

use atlas::{
    api::{failure, nosniff},
    config::Config,
    domain::Error,
};
use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};
use tokio::{net::TcpListener, signal};

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
    let database = if config.database_url.is_some() {
        Some(
            atlas::postgis::connection::Database::open(&config)
                .await
                .map_err(|_| "database unavailable")?,
        )
    } else {
        None
    };
    let catalog = database
        .clone()
        .map(|db| atlas::catalog::Catalog::new(db, config.clone()))
        .transpose()
        .map_err(|_| "invalid configuration")?;
    // Probes stay outside authentication: OpenAPI declares them `security: []`.
    let probes = Router::new()
        .route("/api/v1/health", get(|| async { StatusCode::OK }))
        .route(
            "/api/v1/ready",
            get(move || {
                let catalog = catalog.clone();
                async move {
                    match catalog {
                        Some(catalog) if catalog.ready().await.is_ok() => {
                            StatusCode::OK.into_response()
                        }
                        _ => failure(Error::DatabaseUnavailable),
                    }
                }
            }),
        );
    let publications = database
        .clone()
        .map(|db| atlas::services::publications::Publications::new(db, config.clone()))
        .transpose()
        .map_err(|_| "invalid configuration")?;
    let router = match publications {
        Some(service) => Router::new()
            .merge(atlas::api::datasets::router(service.clone()))
            .merge(atlas::api::tiles::router(service.clone()))
            .merge(atlas::api::features::router(service.clone()))
            .merge(atlas::api::export::router(service.clone()))
            .merge(atlas::api::admin::router(
                service,
                config.limits.admin_body_bytes,
            )),
        None => Router::new(),
    };
    let router = router
        .layer(axum::middleware::from_fn_with_state(
            verifier,
            atlas::auth::http::authenticate,
        ))
        .merge(probes)
        .fallback(|| async { failure(Error::NotFound) })
        // Set after every route is added: axum applies it to existing routes and keeps `Allow`.
        .method_not_allowed_fallback(|| async { failure(Error::MethodNotAllowed) })
        .layer(axum::middleware::map_response(nosniff));
    eprintln!("{{\"event\":\"listening\",\"address\":\"{bound}\"}}");
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            tokio::select! {
                _ = interrupt.recv() => {},
                _ = terminate.recv() => {},
            }
            #[cfg(not(unix))]
            let _ = interrupt.await;
            eprintln!("{{\"event\":\"shutdown_started\"}}");
        })
        .await;
    if let Some(database) = database {
        database.close().await;
    }
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
