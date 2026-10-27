use crate::{
    config::Config,
    domain::Error,
    postgis::connection::{Database, error},
};

pub async fn migrate(config: &Config) -> Result<(), Error> {
    let database = Database::open(config).await?;
    let result = async {
        let mut tx = database.begin_catalog().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(657852314)").execute(&mut *tx).await.map_err(error)?;
        sqlx::raw_sql(include_str!("../migrations/0001_catalog.sql")).execute(&mut *tx).await.map_err(error)?;
        sqlx::query("SELECT id, config, metadata, revision, created_at, updated_at FROM atlas_catalog.datasets LIMIT 0").execute(&mut *tx).await.map_err(error)?;
        tx.commit().await.map_err(error)
    }.await;
    database.close().await;
    result
}
