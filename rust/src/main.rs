use std::{env, process::ExitCode};

use atlas::config::Config;
use axum::{Router, http::StatusCode, routing::get};
use tokio::{net::TcpListener, signal};

fn configuration() -> Result<Config, &'static str> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.first().is_none_or(|command| command != "serve") || args.len() % 2 == 0 {
        return Err("usage: atlas serve [--config FILE] [--listen IP:PORT]");
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
    Config::load(path, listen).map_err(|_| "invalid configuration")
}

async fn serve(config: Config) -> Result<(), &'static str> {
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
    let router = Router::new()
        .route("/api/v1/health", get(|| async { StatusCode::OK }))
        .route(
            "/api/v1/ready",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        );
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
        Ok(config) => serve(config).await,
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
