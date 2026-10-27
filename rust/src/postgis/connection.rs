use crate::{config::Config, domain::Error};
use sqlx::{
    ConnectOptions, PgPool, Postgres, Transaction,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::time::Duration;

pub fn error(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::PoolTimedOut => Error::CapacityExceeded,
        sqlx::Error::Database(ref database) if database.code().as_deref() == Some("57014") => {
            Error::QueryTimeout
        }
        sqlx::Error::Database(ref database) if database.is_unique_violation() => {
            Error::PublicationConflict
        }
        _ => Error::DatabaseUnavailable,
    }
}

#[derive(Clone)]
pub struct Database {
    pool: PgPool,
    timeout_ms: String,
}

impl Database {
    pub async fn open(config: &Config) -> Result<Self, Error> {
        config.validate()?;
        let dsn = config
            .database_url
            .as_deref()
            .ok_or(Error::InvalidRequest)?;
        let timeout_ms = (config.limits.query_seconds * 1000).to_string();
        let options: PgConnectOptions = dsn.parse().map_err(|_| Error::InvalidRequest)?;
        let options = options
            .disable_statement_logging()
            .application_name("atlas-rust")
            .options([("statement_timeout", timeout_ms.as_str())]);
        let pool = PgPoolOptions::new()
            .min_connections(config.limits.pool_min)
            .max_connections(config.limits.pool_max)
            .acquire_timeout(Duration::from_secs(config.limits.acquire_seconds.into()))
            .connect_with(options)
            .await
            .map_err(error)?;
        let database = Self { pool, timeout_ms };
        let mut tx = database.begin_read().await?;
        sqlx::query("SELECT postgis_version()")
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        tx.rollback().await.map_err(error)?;
        Ok(database)
    }

    async fn begin(&self, read_only: bool) -> Result<Transaction<'static, Postgres>, Error> {
        let mut tx = self.pool.begin().await.map_err(error)?;
        let mode = if read_only {
            "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY"
        } else {
            "SET TRANSACTION READ WRITE"
        };
        sqlx::query(mode).execute(&mut *tx).await.map_err(error)?;
        sqlx::query("SELECT set_config('statement_timeout', $1, true)")
            .bind(&self.timeout_ms)
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        Ok(tx)
    }

    pub async fn begin_read(&self) -> Result<Transaction<'static, Postgres>, Error> {
        self.begin(true).await
    }
    pub async fn begin_catalog(&self) -> Result<Transaction<'static, Postgres>, Error> {
        self.begin(false).await
    }
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
