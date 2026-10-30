use crate::{
    config::Config,
    domain::{Dataset, Error, Metadata},
    postgis::{
        connection::{Database, error},
        introspection::inspect,
    },
};
use sqlx::{Row, postgres::PgRow, types::Json};

pub struct Publication {
    pub config: Dataset,
    pub metadata: Metadata,
    pub revision: i64,
}

#[derive(Clone)]
pub struct Catalog {
    database: Database,
    config: Config,
}

impl Catalog {
    pub async fn ready(&self) -> Result<(), Error> {
        let mut tx = self.database.begin_read().await?;
        sqlx::query("SELECT id,config,metadata,revision FROM atlas_catalog.datasets LIMIT 0")
            .execute(&mut *tx)
            .await
            .map_err(error)?;
        tx.rollback().await.map_err(error)
    }
    pub fn new(database: Database, config: Config) -> Result<Self, Error> {
        config.validate()?;
        Ok(Self { database, config })
    }

    fn decode(&self, row: PgRow) -> Result<Publication, Error> {
        let Json(config): Json<Dataset> =
            row.try_get("config").map_err(|_| Error::SchemaChanged)?;
        let Json(metadata): Json<Metadata> =
            row.try_get("metadata").map_err(|_| Error::SchemaChanged)?;
        let id: String = row.try_get("id").map_err(error)?;
        let revision: i64 = row.try_get("revision").map_err(error)?;
        config
            .validate(&self.config.allowed_schemas)
            .map_err(|_| Error::SchemaChanged)?;
        if id != config.id
            || revision <= 0
            || metadata.srid <= 0
            || ![
                "POINT",
                "MULTIPOINT",
                "LINESTRING",
                "MULTILINESTRING",
                "POLYGON",
                "MULTIPOLYGON",
                "GEOMETRYCOLLECTION",
            ]
            .contains(&metadata.geometry_type.as_str())
            || !crate::domain::identifier(&metadata.geometry_column)
            || !crate::domain::identifier(&metadata.id_column)
            || metadata
                .attributes
                .iter()
                .map(|a| &a.name)
                .collect::<Vec<_>>()
                != config.properties.iter().collect::<Vec<_>>()
            || metadata.attributes.iter().any(|a| {
                a.name == metadata.geometry_column
                    || ![
                        "int2",
                        "int4",
                        "int8",
                        "float4",
                        "float8",
                        "numeric",
                        "bool",
                        "text",
                        "varchar",
                        "bpchar",
                        "date",
                        "timestamp",
                        "timestamptz",
                    ]
                    .contains(&a.data_type.as_str())
            })
        {
            return Err(Error::SchemaChanged);
        }
        Ok(Publication {
            config,
            metadata,
            revision,
        })
    }

    pub async fn get(&self, id: &str) -> Result<Publication, Error> {
        let mut tx = self.database.begin_read().await?;
        let row = sqlx::query(
            "SELECT id, config, metadata, revision FROM atlas_catalog.datasets WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?
        .ok_or(Error::DatasetNotFound)?;
        let record = self.decode(row)?;
        tx.rollback().await.map_err(error)?;
        Ok(record)
    }

    pub async fn list(&self, limit: u32, offset: u32) -> Result<Vec<Publication>, Error> {
        let rows = self.rows(limit, offset).await?;
        rows.into_iter().map(|row| self.decode(row)).collect()
    }

    pub async fn servable(&self) -> Result<Vec<Publication>, Error> {
        let rows = self.rows(u32::MAX, 0).await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| self.decode(row).ok())
            .collect())
    }

    async fn rows(&self, limit: u32, offset: u32) -> Result<Vec<PgRow>, Error> {
        let mut tx = self.database.begin_read().await?;
        let rows = sqlx::query("SELECT id, config, metadata, revision FROM atlas_catalog.datasets ORDER BY id LIMIT $1 OFFSET $2")
            .bind(i64::from(limit))
            .bind(i64::from(offset))
            .fetch_all(&mut *tx)
            .await
            .map_err(error)?;
        tx.rollback().await.map_err(error)?;
        Ok(rows)
    }

    pub async fn create(&self, dataset: &Dataset) -> Result<Publication, Error> {
        let metadata = inspect(&self.database, &self.config, dataset).await?;
        let mut tx = self.database.begin_catalog().await?;
        let row = sqlx::query("INSERT INTO atlas_catalog.datasets(id,config,metadata) VALUES ($1,$2,$3) RETURNING id,config,metadata,revision")
            .bind(&dataset.id).bind(Json(dataset)).bind(Json(metadata)).fetch_one(&mut *tx).await.map_err(error)?;
        let record = self.decode(row)?;
        tx.commit().await.map_err(error)?;
        Ok(record)
    }

    pub async fn update(&self, dataset: &Dataset, revision: i64) -> Result<Publication, Error> {
        if revision <= 0 {
            return Err(Error::InvalidRequest);
        }
        let metadata = inspect(&self.database, &self.config, dataset).await?;
        let mut tx = self.database.begin_catalog().await?;
        let row = sqlx::query("UPDATE atlas_catalog.datasets SET config=$1, metadata=$2, revision=revision+1, updated_at=now() WHERE id=$3 AND revision=$4 RETURNING id,config,metadata,revision")
            .bind(Json(dataset)).bind(Json(metadata)).bind(&dataset.id).bind(revision).fetch_optional(&mut *tx).await.map_err(error)?;
        let Some(row) = row else {
            return Err(conflict(&mut tx, &dataset.id).await);
        };
        let record = self.decode(row)?;
        tx.commit().await.map_err(error)?;
        Ok(record)
    }

    pub async fn delete(&self, id: &str, revision: i64) -> Result<(), Error> {
        if revision <= 0 {
            return Err(Error::InvalidRequest);
        }
        let mut tx = self.database.begin_catalog().await?;
        if sqlx::query("DELETE FROM atlas_catalog.datasets WHERE id=$1 AND revision=$2")
            .bind(id)
            .bind(revision)
            .execute(&mut *tx)
            .await
            .map_err(error)?
            .rows_affected()
            == 0
        {
            return Err(conflict(&mut tx, id).await);
        }
        tx.commit().await.map_err(error)
    }
}

async fn conflict(tx: &mut sqlx::PgConnection, id: &str) -> Error {
    match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM atlas_catalog.datasets WHERE id=$1)",
    )
    .bind(id)
    .fetch_one(tx)
    .await
    {
        Ok(true) => Error::RevisionMismatch,
        Ok(false) => Error::DatasetNotFound,
        Err(err) => error(err),
    }
}
