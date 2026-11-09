use atlas::{domain::Error, services::admission::Admission};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

#[tokio::test]
async fn cancelled_callers_keep_slots_until_workers_finish() {
    let admission = Admission::new(8, 1);
    let active = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let mut callers = Vec::new();
    for _ in 0..8 {
        let (gate, count, release) = (admission.clone(), active.clone(), release.clone());
        let permit = gate.acquire().unwrap();
        callers.push(tokio::spawn(async move {
            gate.wait(async move {
                let _permit = permit;
                count.fetch_add(1, Ordering::SeqCst);
                release.notified().await;
                count.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
            .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        while active.load(Ordering::SeqCst) != 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for caller in callers {
        caller.abort();
    }
    assert_eq!(admission.acquire().err(), Some(Error::CapacityExceeded));
    assert_eq!(active.load(Ordering::SeqCst), 8);
    release.notify_waiters();
    tokio::time::timeout(Duration::from_secs(10), async {
        while active.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let permit = admission.acquire().unwrap();
    let unblock = Arc::new(Notify::new());
    let waiting = unblock.clone();
    assert_eq!(
        admission
            .wait(async move {
                let _permit = permit;
                waiting.notified().await;
                Ok(())
            })
            .await
            .err(),
        Some(Error::QueryTimeout)
    );
    // Seven other slots remain usable while the timed-out worker drains.
    let held: Vec<_> = (0..7).map(|_| admission.acquire().unwrap()).collect();
    assert_eq!(admission.acquire().err(), Some(Error::CapacityExceeded));
    drop(held);
    unblock.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let permits: Vec<_> = (0..8).filter_map(|_| admission.acquire().ok()).collect();
            if permits.len() == 8 {
                break;
            }
            drop(permits);
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn postgis_cancellation_and_stalled_http_release_slots() {
    use atlas::{
        config::Config,
        domain::{Principal, Query},
        migrations::migrate,
        postgis::connection::Database,
        services::publications::Publications,
    };
    use std::io::{Read, Write};
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").unwrap()),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["admission_test".into()];
    config.limits.data_operations = 2;
    config.limits.pool_max = 4;
    config.limits.query_seconds = 30;
    config.limits.stream_seconds = 1;
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS admission_test CASCADE",
        "CREATE SCHEMA admission_test",
        "DELETE FROM atlas_catalog.datasets WHERE id LIKE 'admission_%'",
        "CREATE TABLE admission_test.data(id integer PRIMARY KEY, value text, geom geometry(Point,4326))",
        "INSERT INTO admission_test.data SELECT n, 'small', ST_SetSRID(ST_Point(0,0),4326) FROM generate_series(1,3) n",
        "CREATE TABLE admission_test.big AS SELECT n AS id, repeat('x',65536) AS value, ST_SetSRID(ST_Point(0,0),4326) AS geom FROM generate_series(1,256) n",
        "ALTER TABLE admission_test.big ADD PRIMARY KEY(id)",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db.clone(), config).unwrap();
    for (id, table) in [("admission_small", "data"), ("admission_big", "big")] {
        let body = format!(
            r#"{{"id":"{id}","title":"X","source":{{"schema":"admission_test","table":"{table}"}},"properties":["value"],"access":{{"public":true,"public_download":true}}}}"#
        );
        service.create(body.as_bytes()).await.unwrap();
    }
    let publication = service.get("admission_small").await.unwrap();
    let mut locker = db.begin_catalog().await.unwrap();
    sqlx::query("LOCK admission_test.data IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *locker)
        .await
        .unwrap();
    let mut callers = Vec::new();
    for _ in 0..2 {
        let (service, publication) = (service.clone(), publication.clone());
        callers.push(tokio::spawn(async move {
            service.features(&publication, &Query::default()).await
        }));
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut tx = db.begin_catalog().await.unwrap();
            let blocked: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name='atlas-rust' AND wait_event_type='Lock' AND query LIKE '%admission_test%'").fetch_one(&mut *tx).await.unwrap();
            tx.rollback().await.unwrap(); if blocked == 2 { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    for caller in callers {
        caller.abort();
    }
    assert_eq!(
        service
            .features(&publication, &Query::default())
            .await
            .err(),
        Some(Error::CapacityExceeded)
    );
    assert_eq!(
        service
            .tile(&publication, (0, 0, 0), None, None)
            .await
            .err(),
        Some(Error::CapacityExceeded)
    );
    assert_eq!(
        service
            .export("admission_small", &Principal::default(), &Query::default())
            .await
            .err(),
        Some(Error::CapacityExceeded)
    );
    // Catalog operations stay available while both data workers are blocked.
    assert!(
        service
            .visible(&Principal::default(), Some(1), 0)
            .await
            .is_ok()
    );
    locker.rollback().await.unwrap();
    wait_for_features(&service, &publication).await;
    let mut exports = Vec::new();
    for _ in 0..2 {
        exports.push(
            service
                .export("admission_small", &Principal::default(), &Query::default())
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        service
            .features(&publication, &Query::default())
            .await
            .err(),
        Some(Error::CapacityExceeded)
    );
    drop(exports);
    wait_for_features(&service, &publication).await;
    let app = atlas::api::export::router(service.clone())
        .route("/api/v1/health", axum::routing::get(|| async { "ok" }))
        .layer(axum::Extension(Principal::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let socket = tokio::task::spawn_blocking(move || {
        let mut s = std::net::TcpStream::connect(address).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(
            b"GET /api/v1/datasets/admission_big/export HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .unwrap();
        let mut head = [0; 128];
        s.read_exact(&mut head).unwrap();
        assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"));
        s
    })
    .await
    .unwrap();
    // The client is still connected and not reading, but the producer's deadline drained its lease.
    let mut again = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            while again.len() < 2 {
                match service
                    .export("admission_small", &Principal::default(), &Query::default())
                    .await
                {
                    Ok(export) => again.push(export),
                    Err(Error::CapacityExceeded) => break,
                    Err(error) => panic!("{error}"),
                }
            }
            if again.len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let health = tokio::task::spawn_blocking(move || {
        let mut s = std::net::TcpStream::connect(address).unwrap();
        s.write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut text = String::new();
        s.read_to_string(&mut text).unwrap();
        text
    })
    .await
    .unwrap();
    assert!(health.starts_with("HTTP/1.1 200"));
    drop(again);
    drop(socket);
    server.abort();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut tx = db.begin_catalog().await.unwrap();
            let active: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name='atlas-rust' AND pid <> pg_backend_pid() AND state IN ('active','idle in transaction')").fetch_one(&mut *tx).await.unwrap();
            tx.rollback().await.unwrap();
            if active == 0 { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    db.close().await;
}

async fn wait_for_features(
    service: &atlas::services::publications::Publications,
    publication: &atlas::catalog::Publication,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match service
                .features(publication, &atlas::domain::Query::default())
                .await
            {
                Ok(_) => break,
                Err(Error::CapacityExceeded) => tokio::task::yield_now().await,
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .unwrap();
}
