use crate::{
    catalog::{Catalog, Publication},
    config::Config,
    domain::{Dataset, Error},
    postgis::{connection::Database, introspection::inspect, selection::prepare},
};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct Publications {
    catalog: Catalog,
    database: Database,
    config: Config,
}

impl Publications {
    pub fn new(database: Database, config: Config) -> Result<Self, Error> {
        Ok(Self {
            catalog: Catalog::new(database.clone(), config.clone())?,
            database,
            config,
        })
    }

    pub async fn list(
        &self,
        limit: Option<u32>,
        offset: u32,
    ) -> Result<(Vec<Publication>, bool), Error> {
        let limits = &self.config.limits;
        let limit = limit.unwrap_or(limits.catalog_default);
        if limit == 0 || limit > limits.catalog_max || offset > limits.max_offset {
            return Err(Error::InvalidRequest);
        }
        let mut page = self.catalog.list(limit + 1, offset).await?;
        let more = page.len() > limit as usize;
        page.truncate(limit as usize);
        Ok((page, more))
    }

    pub async fn get(&self, id: &str) -> Result<Publication, Error> {
        self.catalog.get(id).await
    }

    pub async fn create(&self, body: &[u8]) -> Result<Publication, Error> {
        let dataset: Dataset = serde_json::from_slice(body).map_err(|_| Error::InvalidRequest)?;
        self.check(&dataset).await?;
        self.catalog.create(&dataset).await
    }

    pub async fn delete(&self, id: &str, revision: i64) -> Result<(), Error> {
        self.catalog.delete(id, revision).await
    }

    async fn check(&self, dataset: &Dataset) -> Result<(), Error> {
        dataset.validate(&self.config.allowed_schemas)?;
        let meta = inspect(&self.database, &self.config, dataset)
            .await
            .map_err(|e| match e {
                Error::DatasetNotFound => Error::InvalidRequest,
                other => other,
            })?;
        let mut tx = self.database.begin_read().await?;
        let fixed = dataset.filter.as_ref().map(|filter| filter.get());
        prepare(&mut tx, fixed, None, None, &meta, &self.config.limits).await?;
        Ok(())
    }
}

pub fn document(publication: &Publication) -> Value {
    let (config, meta) = (&publication.config, &publication.metadata);
    let attributes: Vec<_> = meta
        .attributes
        .iter()
        .map(|a| json!({"name": a.name, "type": a.data_type, "nullable": a.nullable}))
        .collect();
    json!({"id": config.id, "config": config, "revision": publication.revision,
        "metadata": {"id": config.id, "title": config.title, "description": config.description,
            "geometry_type": meta.geometry_type, "srid": meta.srid, "attributes": attributes,
            "formats": ["geojson", "mvt"]}})
}
