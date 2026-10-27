use atlas::{
    config::Config,
    domain::Error,
    postgis::connection::{Database, error},
};
use std::time::{Duration, Instant};

#[tokio::test]
async fn timeout_saturation_rollback_cancel_and_close() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let config = Config::parse(
        "[limits]\npool_max=1\nacquire_seconds=1\nquery_seconds=1",
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
    assert_eq!(error(err), Error::DatabaseUnavailable);
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
