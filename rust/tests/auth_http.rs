use atlas::{
    auth::{http::authenticate, jwt::Verifier},
    domain::Principal,
};
use axum::{Extension, Router, middleware, routing::get};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, get_current_timestamp};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[test]
fn real_http_authenticates_before_handlers() {
    for configured in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let verifier = configured.then(|| {
            Verifier::new(
                include_bytes!("fixtures/jwt-test-public.pem"),
                "atlas-test",
                "atlas",
            )
            .unwrap()
        });
        let router = Router::new()
            .route(
                "/public",
                get(move |Extension(p): Extension<Principal>| {
                    let count = count.clone();
                    async move {
                        count.fetch_add(1, Ordering::Relaxed);
                        p.sub.unwrap_or_else(|| "anonymous".into())
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(
                Arc::new(verifier),
                authenticate,
            ));
        let (send, recv) = std::sync::mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async move {
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    send.send(listener.local_addr().unwrap()).unwrap();
                    axum::serve(listener, router)
                        .with_graceful_shutdown(async {
                            let _ = stopped.await;
                        })
                        .await
                        .unwrap();
                });
        });
        let address = recv.recv_timeout(Duration::from_secs(5)).unwrap();
        let c = serde_json::json!({"sub":"alice", "iss":"atlas-test", "aud":"atlas", "exp":get_current_timestamp()+3600});
        let token = encode(
            &Header::new(Algorithm::RS256),
            &c,
            &EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwt-test-private.pem")).unwrap(),
        )
        .unwrap();
        for (path, header, expected) in [
            ("/public", String::new(), "200"),
            (
                "/public",
                format!("Authorization: Bearer {token}\r\n"),
                if configured { "200" } else { "401" },
            ),
            (
                "/public",
                format!("Authorization: bearer {token}\r\n"),
                if configured { "200" } else { "401" },
            ),
            (
                "/public",
                "Authorization: Bearer secret-token\r\n".into(),
                "401",
            ),
            (
                "/public",
                "Authorization: Basic secret-token\r\n".into(),
                "401",
            ),
            ("/public", "Authorization: Bearer\r\n".into(), "401"),
            (
                "/public",
                "Authorization: Bearer  secret-token\r\n".into(),
                "401",
            ),
            (
                "/public",
                format!("Authorization: Bearer {token}\r\nAuthorization: Bearer {token}\r\n"),
                "401",
            ),
            ("/public?access_token=secret-token", String::new(), "200"),
        ] {
            let before = calls.load(Ordering::Relaxed);
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            write!(
                stream,
                "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{header}\r\n"
            )
            .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(
                response.starts_with(&format!("HTTP/1.1 {expected}")),
                "{response}"
            );
            assert!(!response.contains("secret-token") && !response.contains(&token));
            if expected == "401" {
                assert!(response.contains("invalid_token") && response.contains("request_id"));
                assert_eq!(calls.load(Ordering::Relaxed), before);
            } else {
                assert!(response.ends_with(if header.is_empty() {
                    "anonymous"
                } else {
                    "alice"
                }));
            }
        }
        stop.send(()).unwrap();
        server.join().unwrap();
    }
}
