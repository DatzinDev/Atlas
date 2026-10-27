use std::{env, net::SocketAddr, process::ExitCode};

use axum::{Router, http::StatusCode, routing::get};
use tokio::{net::TcpListener, signal};

fn listen_address() -> Result<SocketAddr, &'static str> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.as_slice() {
        [command] if command == "serve" => Ok(([127, 0, 0, 1], 8080).into()),
        [command, option, value] if command == "serve" && option == "--listen" => value
            .to_str()
            .and_then(|value| value.parse().ok())
            .ok_or("invalid listen address"),
        _ => Err("usage: atlas serve [--listen IP:PORT]"),
    }
}

async fn serve(address: SocketAddr) -> Result<(), &'static str> {
    // Register before accepting connections so an early signal cannot bypass draining.
    #[cfg(unix)]
    let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())
        .map_err(|_| "cannot register shutdown signal")?;
    #[cfg(unix)]
    let mut interrupt = signal::unix::signal(signal::unix::SignalKind::interrupt())
        .map_err(|_| "cannot register shutdown signal")?;
    #[cfg(not(unix))]
    let interrupt = signal::ctrl_c();
    let listener = TcpListener::bind(address)
        .await
        .map_err(|_| "cannot bind listener")?;
    let bound = listener
        .local_addr()
        .map_err(|_| "cannot inspect listener")?;
    let router = Router::new()
        .route("/api/v1/health", get(|| async { StatusCode::OK }))
        .route(
            "/api/v1/ready",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        );
    eprintln!("{{\"event\":\"listening\",\"address\":\"{bound}\"}}");
    axum::serve(listener, router)
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
        .await
        .map_err(|_| "server failed")?;
    eprintln!("{{\"event\":\"shutdown_complete\"}}");
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let result = match listen_address() {
        Ok(address) => serve(address).await,
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
