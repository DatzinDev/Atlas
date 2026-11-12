use atlas::{domain::Error, services::admission::Admission};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

// PostGIS tests share pg_stat_activity accounting, so they must not overlap.
static POSTGIS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const ACTIVE_SESSIONS: &str = "SELECT count(*) FROM pg_stat_activity WHERE application_name='atlas-rust' AND pid <> pg_backend_pid() AND state IN ('active','idle in transaction')";

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
        tokio::time::timeout(
            Duration::from_secs(10),
            admission.wait(async move {
                let _permit = permit;
                waiting.notified().await;
                Ok(())
            }),
        )
        .await
        .unwrap()
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
    let _serial = POSTGIS.lock().await;
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
    config.limits.stream_seconds = 3;
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
    // Both aborted workers must drain before the two slots are free again.
    let mut exports = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            while exports.len() < 2 {
                match service
                    .export("admission_small", &Principal::default(), &Query::default())
                    .await
                {
                    Ok(export) => exports.push(export),
                    Err(Error::CapacityExceeded) => break,
                    Err(error) => panic!("{error}"),
                }
            }
            if exports.len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
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
            let active: i64 = sqlx::query_scalar(ACTIVE_SESSIONS)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
            if active == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
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

fn http(address: std::net::SocketAddr, target: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(address).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = String::new();
    s.read_to_string(&mut text).unwrap();
    text
}

async fn get(address: std::net::SocketAddr, target: &'static str) -> String {
    tokio::task::spawn_blocking(move || http(address, target))
        .await
        .unwrap()
}

/// Retries while the response is a 503 (a slot still draining) and returns the first other one.
async fn get_admitted(address: std::net::SocketAddr, target: &'static str) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let response = get(address, target).await;
            if !response.starts_with("HTTP/1.1 503") {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn stalled_exports_cannot_starve_items_and_tiles() {
    use atlas::{
        config::Config,
        domain::{Principal, Query},
        migrations::migrate,
        postgis::connection::Database,
        services::publications::Publications,
    };
    use std::io::{Read, Write};
    let _serial = POSTGIS.lock().await;
    let mut config = Config::parse(
        "[limits]\ndata_operations=3\nexport_operations=1\npool_max=6\nstream_seconds=120",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").unwrap()),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["admission_fair".into()];
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS admission_fair CASCADE",
        "CREATE SCHEMA admission_fair",
        "DELETE FROM atlas_catalog.datasets WHERE id LIKE 'fairness_%'",
        "CREATE TABLE admission_fair.data(id integer PRIMARY KEY, value text, geom geometry(Point,4326))",
        "INSERT INTO admission_fair.data SELECT n, 'small', ST_SetSRID(ST_Point(0,0),4326) FROM generate_series(1,3) n",
        "CREATE TABLE admission_fair.big AS SELECT n AS id, repeat('x',65536) AS value, ST_SetSRID(ST_Point(0,0),4326) AS geom FROM generate_series(1,256) n",
        "ALTER TABLE admission_fair.big ADD PRIMARY KEY(id)",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db.clone(), config).unwrap();
    for (id, table) in [("fairness_small", "data"), ("fairness_big", "big")] {
        let body = format!(
            r#"{{"id":"{id}","title":"X","source":{{"schema":"admission_fair","table":"{table}"}},"properties":["value"],"access":{{"public":true,"public_download":true}}}}"#
        );
        service.create(body.as_bytes()).await.unwrap();
    }
    let publication = service.get("fairness_small").await.unwrap();
    let app = atlas::api::export::router(service.clone())
        .merge(atlas::api::features::router(service.clone()))
        .layer(axum::Extension(Principal::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // A public download whose client never reads keeps its export open (16 MiB > socket buffers).
    let stalled = tokio::task::spawn_blocking(move || {
        let mut s = std::net::TcpStream::connect(address).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(
            b"GET /api/v1/datasets/fairness_big/export HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .unwrap();
        let mut head = [0; 128];
        s.read_exact(&mut head).unwrap();
        assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"));
        s
    })
    .await
    .unwrap();

    // The only export slot is taken: a second export is refused at once, without queueing.
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        get(address, "/api/v1/datasets/fairness_small/export"),
    )
    .await
    .unwrap();
    assert!(refused.starts_with("HTTP/1.1 503"), "{refused}");
    assert!(
        refused.contains(r#""code":"capacity_exceeded""#),
        "{refused}"
    );
    assert!(!refused.contains("application/geo+json"), "{refused}");
    assert_eq!(
        service
            .export("fairness_small", &Principal::default(), &Query::default())
            .await
            .err(),
        Some(Error::CapacityExceeded)
    );

    // Items and tiles keep data_operations - export_operations slots.
    let (first, second) = tokio::join!(
        get(address, "/api/v1/datasets/fairness_small/items"),
        get(address, "/api/v1/datasets/fairness_small/items"),
    );
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    assert!(second.starts_with("HTTP/1.1 200"), "{second}");
    assert!(
        service
            .tile(&publication, (0, 0, 0), None, None)
            .await
            .is_ok()
    );

    // Two in-flight items fill the remaining data slots: the open export still holds one.
    let mut locker = db.begin_catalog().await.unwrap();
    sqlx::query("LOCK admission_fair.data IN ACCESS EXCLUSIVE MODE")
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
            let blocked: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name='atlas-rust' AND wait_event_type='Lock' AND query LIKE '%admission_fair%'").fetch_one(&mut *tx).await.unwrap();
            tx.rollback().await.unwrap();
            if blocked == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        service
            .features(&publication, &Query::default())
            .await
            .err(),
        Some(Error::CapacityExceeded)
    );
    locker.rollback().await.unwrap();
    for caller in callers {
        assert!(caller.await.unwrap().is_ok());
    }
    wait_for_features(&service, &publication).await;

    // Disconnecting the stalled client cancels its export and frees both of its slots.
    drop(stalled);
    let finished = get_admitted(address, "/api/v1/datasets/fairness_small/export").await;
    assert!(finished.starts_with("HTTP/1.1 200"), "{finished}");
    assert!(finished.ends_with("0\r\n\r\n"), "{finished}");

    // A download that ran to completion frees its export slot as well.
    let again = get_admitted(address, "/api/v1/datasets/fairness_small/export").await;
    assert!(again.starts_with("HTTP/1.1 200"), "{again}");
    let mut export = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match service
                .export("fairness_small", &Principal::default(), &Query::default())
                .await
            {
                Ok(export) => break export,
                Err(Error::CapacityExceeded) => tokio::task::yield_now().await,
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .unwrap();
    while export.next().await.unwrap().is_some() {}
    drop(export);
    let last = get_admitted(address, "/api/v1/datasets/fairness_small/export").await;
    assert!(last.starts_with("HTTP/1.1 200"), "{last}");
    server.abort();
    db.close().await;
}

#[tokio::test]
async fn catalog_traffic_cannot_take_connections_from_admitted_work() {
    use atlas::{
        catalog::Catalog, config::Config, domain::Principal, migrations::migrate,
        postgis::connection::Database, services::publications::Publications,
    };
    use sqlx::Connection;
    let _serial = POSTGIS.lock().await;
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").unwrap();
    // One connection is left for catalog work: pool_max - data_operations = 1.
    let mut config = Config::parse(
        "[limits]\npool_max=3\ndata_operations=2\nexport_operations=1\nacquire_seconds=1",
        Some(dsn.clone()),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["admission_reserve".into()];
    migrate(&config).await.unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS admission_reserve CASCADE",
        "CREATE SCHEMA admission_reserve",
        "DELETE FROM atlas_catalog.datasets WHERE id LIKE 'reservation_%'",
        "CREATE TABLE admission_reserve.data(id integer PRIMARY KEY, value text, geom geometry(Point,4326))",
        "INSERT INTO admission_reserve.data VALUES (1,'small',ST_SetSRID(ST_Point(0,0),4326))",
        "CREATE VIEW admission_reserve.slow AS SELECT d.* FROM admission_reserve.data d CROSS JOIN pg_sleep(3)",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let service = Publications::new(db.clone(), config.clone()).unwrap();
    service
        .create(br#"{"id":"reservation_slow","title":"X","source":{"schema":"admission_reserve","table":"slow","id_column":"id"},"properties":["value"],"access":{"public":true}}"#)
        .await
        .unwrap();
    // `/ready` uses its own Catalog over the same Database, as the binary wires it.
    let ready = Catalog::new(db.clone(), config.clone()).unwrap();
    let app = atlas::api::datasets::router(service.clone())
        .merge(atlas::api::features::router(service.clone()))
        .route(
            "/api/v1/ready",
            axum::routing::get(move || {
                let ready = ready.clone();
                async move {
                    let ready = ready.ready().await;
                    ready.map_or(axum::http::StatusCode::SERVICE_UNAVAILABLE, |()| {
                        axum::http::StatusCode::OK
                    })
                }
            }),
        )
        .layer(axum::Extension(Principal::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    // Observe through a connection outside Atlas' pool.
    let mut observer = sqlx::PgConnection::connect(&dsn).await.unwrap();

    // Two slow items hold both data slots while a burst of catalog requests arrives.
    let items: Vec<_> = (0..2)
        .map(|_| tokio::spawn(get(address, "/api/v1/datasets/reservation_slow/items")))
        .collect();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let sleeping: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name='atlas-rust' AND wait_event='PgSleep'")
                .fetch_one(&mut observer)
                .await
                .unwrap();
            if sleeping == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let burst: Vec<_> = (0..10)
        .map(|n| tokio::spawn(get(address, ["/api/v1/datasets", "/api/v1/ready"][n % 2])))
        .collect();
    for response in burst {
        let response = response.await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    }
    for item in items {
        let response = item.await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    }
    server.abort();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DELETE FROM atlas_catalog.datasets WHERE id LIKE 'reservation_%'",
        "DROP SCHEMA admission_reserve CASCADE",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    db.close().await;
}
