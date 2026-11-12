use crate::{
    config::Config,
    domain::Error,
    postgis::connection::{Database, error},
};

const MIGRATIONS: [(i32, &str); 3] = [
    (1, include_str!("../migrations/0001_catalog.sql")),
    (2, include_str!("../migrations/0002_metadata_id_type.sql")),
    (3, include_str!("../migrations/0003_versions.sql")),
];
/// Latest embedded catalog schema version; `/ready` requires exactly this one.
pub const CATALOG_VERSION: i32 = MIGRATIONS[MIGRATIONS.len() - 1].0;

pub async fn migrate(config: &Config) -> Result<(), Error> {
    let database = Database::open(config).await?;
    let result = async {
        let mut tx = database.begin_catalog().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(657852314)").execute(&mut *tx).await.map_err(error)?;
        let tracked: bool = sqlx::query_scalar("SELECT to_regclass('atlas_catalog.migrations') IS NOT NULL")
            .fetch_one(&mut *tx)
            .await
            .map_err(error)?;
        let applied: Vec<i32> = if tracked {
            sqlx::query_scalar("SELECT version FROM atlas_catalog.migrations")
                .fetch_all(&mut *tx)
                .await
                .map_err(error)?
        } else {
            Vec::new()
        };
        let mut pending = Vec::new();
        for (version, sql) in MIGRATIONS {
            if !applied.contains(&version) {
                sqlx::raw_sql(sql).execute(&mut *tx).await.map_err(error)?;
                pending.push(version);
            }
        }
        // The versions table only exists after 0003 ran, so versions are recorded last.
        sqlx::query("INSERT INTO atlas_catalog.migrations (version) SELECT unnest($1::integer[])")
            .bind(pending)
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        sqlx::query("SELECT id, config, metadata, revision, created_at, updated_at FROM atlas_catalog.datasets LIMIT 0").execute(&mut *tx).await.map_err(error)?;
        tx.commit().await.map_err(error)
    }.await;
    database.close().await;
    result
}
