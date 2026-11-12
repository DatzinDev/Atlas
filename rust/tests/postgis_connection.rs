use atlas::{
    config::Config,
    domain::Error,
    postgis::{
        connection::{Database, error},
        predicates::query_error,
    },
};
use std::time::{Duration, Instant};

// R15
#[tokio::test]
async fn timeout_saturation_rollback_cancel_and_close() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let config = Config::parse(
        "[limits]\npool_max=2\ndata_operations=1\nexport_operations=1\nacquire_seconds=1\nquery_seconds=1",
        Some(dsn),
        None,
    )
    .unwrap();
    let database = Database::open(&config).await.unwrap();
    let mut tx = database.begin_read().await.unwrap();
    let readonly: String = sqlx::query_scalar("SHOW transaction_read_only")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(readonly, "on");
    assert!(matches!(
        database.begin_read().await,
        Err(Error::CapacityExceeded)
    ));
    let err = sqlx::query("SELECT pg_sleep(5)")
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(error(err), Error::QueryTimeout);
    drop(tx);
    let mut tx = database.begin_catalog().await.unwrap();
    sqlx::query("CREATE TEMP TABLE rolled_back(id integer)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = sqlx::query("SELECT 1/0")
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(error(err), Error::InternalError);
    drop(tx);
    let mut tx = database.begin_read().await.unwrap();
    let absent: bool = sqlx::query_scalar("SELECT to_regclass('pg_temp.rolled_back') IS NULL")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(absent);
    drop(tx);
    let cloned = database.clone();
    let (started, running) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut tx = cloned.begin_read().await.unwrap();
        started.send(()).unwrap();
        sqlx::query("SELECT pg_sleep(5)").execute(&mut *tx).await
    });
    running.await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let start = Instant::now();
    task.abort();
    let _ = task.await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let mut tx = database.begin_read().await.unwrap();
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name='atlas-rust' AND state='active' AND pid <> pg_backend_pid()").fetch_one(&mut *tx).await.unwrap();
    assert_eq!(active, 0);
    assert!(start.elapsed() < Duration::from_secs(3));
    drop(tx);
    let mut tx = database.begin_catalog().await.unwrap();
    assert!(
        sqlx::query("SELECT pg_terminate_backend(pg_backend_pid())")
            .execute(&mut *tx)
            .await
            .is_err()
    );
    drop(tx);
    let tx = database.begin_read().await.unwrap();
    drop(tx);
    database.close().await;
    assert!(matches!(
        database.begin_read().await,
        Err(Error::DatabaseUnavailable)
    ));
}

async fn raised(database: &Database, code: &str) -> sqlx::Error {
    let mut tx = database.begin_catalog().await.unwrap();
    let statement =
        format!("DO $$BEGIN RAISE EXCEPTION 'leaked-detail' USING ERRCODE = '{code}'; END$$");
    // Safe: `code` only ever comes from fixed SQLSTATE literals in this test; DO blocks take no binds.
    let failure = sqlx::query(sqlx::AssertSqlSafe(statement))
        .execute(&mut *tx)
        .await
        .unwrap_err();
    drop(tx);
    failure
}

#[tokio::test]
async fn sqlstate_mapping() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let config = Config::parse("", Some(dsn), None).unwrap();
    let database = Database::open(&config).await.unwrap();
    for (code, expected) in [
        ("42501", Error::InternalError),
        ("42883", Error::InternalError),
        ("XX000", Error::InternalError),
        ("22P02", Error::InternalError),
        ("42703", Error::InternalError),
        ("42P01", Error::InternalError),
        ("P0001", Error::InternalError),
        ("08006", Error::DatabaseUnavailable),
        ("08000", Error::DatabaseUnavailable),
        ("53300", Error::DatabaseUnavailable),
        ("53100", Error::DatabaseUnavailable),
        ("57P01", Error::DatabaseUnavailable),
        ("57P02", Error::DatabaseUnavailable),
        ("57P03", Error::DatabaseUnavailable),
        ("57014", Error::QueryTimeout),
        ("23505", Error::PublicationConflict),
    ] {
        assert_eq!(
            error(raised(&database, code).await),
            expected,
            "error {code}"
        );
    }
    for (code, expected) in [
        ("22P02", Error::InvalidFilter),
        ("22012", Error::InvalidFilter),
        ("42703", Error::SchemaChanged),
        ("42P01", Error::SchemaChanged),
        ("42501", Error::InternalError),
        ("42883", Error::InternalError),
        ("XX000", Error::InternalError),
        ("57014", Error::QueryTimeout),
        ("57P01", Error::DatabaseUnavailable),
    ] {
        assert_eq!(
            query_error(raised(&database, code).await),
            expected,
            "query_error {code}"
        );
    }
    let mut tx = database.begin_read().await.unwrap();
    let decode = sqlx::query_scalar::<_, i32>("SELECT 'x'::text")
        .fetch_one(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(error(decode), Error::InternalError);
    drop(tx);
    assert_eq!(
        error(sqlx::Error::Decode("bad".into())),
        Error::InternalError
    );
    assert_eq!(
        query_error(sqlx::Error::Decode("bad".into())),
        Error::InternalError
    );
    assert_eq!(
        error(sqlx::Error::Io(std::io::Error::other("reset"))),
        Error::DatabaseUnavailable
    );
    assert_eq!(error(sqlx::Error::PoolClosed), Error::DatabaseUnavailable);
    assert_eq!(error(sqlx::Error::PoolTimedOut), Error::CapacityExceeded);
    database.close().await;
}

/// Reads `(current_setting, reset_val)` of `statement_timeout` inside a fresh transaction.
async fn timeouts(tx: &mut sqlx::Transaction<'static, sqlx::Postgres>) -> (String, String) {
    sqlx::query_as(
        "SELECT current_setting('statement_timeout'), \
         (SELECT reset_val FROM pg_settings WHERE name = 'statement_timeout')",
    )
    .fetch_one(&mut **tx)
    .await
    .unwrap()
}

/// Session default as seen by a plain connection with the same DSN (server default plus
/// whatever the URL itself carries), without any Atlas option.
async fn session_default(dsn: &str) -> String {
    use sqlx::Connection;
    let mut connection = sqlx::PgConnection::connect(dsn).await.unwrap();
    let value: String =
        sqlx::query_scalar("SELECT reset_val FROM pg_settings WHERE name = 'statement_timeout'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    connection.close().await.unwrap();
    value
}

#[tokio::test]
async fn statement_timeout_is_transaction_local_without_startup_options() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let server_default = session_default(&dsn).await;
    let config = Config::parse(
        "[limits]\npool_max=2\ndata_operations=1\nexport_operations=1\nquery_seconds=30",
        Some(dsn),
        None,
    )
    .unwrap();
    let database = Database::open(&config).await.unwrap();
    // One reserved connection: every transaction below reuses the same physical session.
    let mut tx = database.begin_read().await.unwrap();
    let (current, reset) = timeouts(&mut tx).await;
    assert_eq!(current, "30s");
    // A startup `options` would turn the session default into 30000.
    assert_eq!(reset, server_default);
    tx.rollback().await.unwrap();
    let mut tx = database.begin_catalog().await.unwrap();
    let (current, reset) = timeouts(&mut tx).await;
    assert_eq!(current, "30s");
    assert_eq!(reset, server_default);
    tx.commit().await.unwrap();
    database.close().await;
}

#[tokio::test]
async fn url_options_are_kept_and_transaction_timeout_still_wins() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let separator = if dsn.contains('?') { '&' } else { '?' };
    let dsn = format!("{dsn}{separator}options=-c%20statement_timeout%3D7s");
    let config = Config::parse(
        "[limits]\npool_max=2\ndata_operations=1\nexport_operations=1\nquery_seconds=30",
        Some(dsn),
        None,
    )
    .unwrap();
    let database = Database::open(&config).await.unwrap();
    let mut tx = database.begin_read().await.unwrap();
    let (current, reset) = timeouts(&mut tx).await;
    assert_eq!(current, "30s");
    // The URL's own startup option is the session default; Atlas adds none on top of it.
    assert_eq!(reset, "7000");
    drop(tx);
    database.close().await;
}
