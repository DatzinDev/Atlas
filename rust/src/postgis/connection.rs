use crate::{config::Config, domain::Error};
use sqlx::{
    ConnectOptions, PgPool, Postgres, Transaction,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::time::Duration;

pub fn error(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::PoolTimedOut => Error::CapacityExceeded,
        sqlx::Error::Database(ref database) => match database.code().as_deref().unwrap_or("") {
            "57014" => Error::QueryTimeout,
            "23505" => Error::PublicationConflict,
            "57P01" | "57P02" | "57P03" => Error::DatabaseUnavailable,
            code if code.starts_with("08") || code.starts_with("53") => Error::DatabaseUnavailable,
            code => {
                eprintln!(
                    "{}",
                    serde_json::json!({"event":"database_error","sqlstate":code})
                );
                Error::InternalError
            }
        },
        sqlx::Error::Decode(_) | sqlx::Error::ColumnDecode { .. } => {
            eprintln!(
                "{}",
                serde_json::json!({"event":"database_error","kind":"decode"})
            );
            Error::InternalError
        }
        _ => Error::DatabaseUnavailable,
    }
}

#[derive(Clone)]
pub struct Database {
    // Admitted work (items, tiles, export) gets `data_operations` connections; catalog,
    // inspection and readiness share the rest, so they never starve admitted work.
    data: PgPool,
    reserved: PgPool,
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
            // Lets the server end backends of aborted work (e.g. a forced shutdown) within ~1 s.
            .options([
                ("statement_timeout", timeout_ms.as_str()),
                ("client_connection_check_interval", "1000"),
            ]);
        let limits = &config.limits;
        let data_min = limits.pool_min.min(limits.data_operations);
        let pool = |min: u32, max: u32| {
            PgPoolOptions::new()
                .min_connections(min)
                .max_connections(max)
                .acquire_timeout(Duration::from_secs(limits.acquire_seconds.into()))
                .connect_with(options.clone())
        };
        let data = pool(data_min, limits.data_operations)
            .await
            .map_err(error)?;
        let reserved = pool(
            limits.pool_min - data_min,
            limits.pool_max - limits.data_operations,
        )
        .await
        .map_err(error)?;
        let database = Self {
            data,
            reserved,
            timeout_ms,
        };
        let mut tx = database.begin_read().await?;
        sqlx::query("SELECT postgis_version()")
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        tx.rollback().await.map_err(error)?;
        Ok(database)
    }

    async fn begin(
        &self,
        pool: &PgPool,
        read_only: bool,
    ) -> Result<Transaction<'static, Postgres>, Error> {
        let mut tx = pool.begin().await.map_err(error)?;
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

    /// Read transaction for work already holding a data admission slot.
    pub async fn begin_data(&self) -> Result<Transaction<'static, Postgres>, Error> {
        self.begin(&self.data, true).await
    }
    pub async fn begin_read(&self) -> Result<Transaction<'static, Postgres>, Error> {
        self.begin(&self.reserved, true).await
    }
    pub async fn begin_catalog(&self) -> Result<Transaction<'static, Postgres>, Error> {
        self.begin(&self.reserved, false).await
    }
    pub async fn close(&self) {
        tokio::join!(self.data.close(), self.reserved.close());
    }
}
