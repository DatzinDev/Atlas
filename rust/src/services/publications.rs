use crate::{
    auth::policy,
    catalog::{Catalog, Publication},
    config::Config,
    domain::{Dataset, Error, Principal},
    postgis::{connection::Database, introspection::inspect, selection::prepare},
};
use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value, json};

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

    pub async fn visible(
        &self,
        principal: &Principal,
        limit: Option<u32>,
        offset: u32,
    ) -> Result<(Vec<Publication>, bool), Error> {
        let limits = &self.config.limits;
        let limit = limit.unwrap_or(limits.catalog_default);
        if limit == 0 || limit > limits.catalog_max || offset > limits.max_offset {
            return Err(Error::InvalidRequest);
        }
        let mut page: Vec<_> = self
            .catalog
            .servable()
            .await?
            .into_iter()
            .filter(|p| policy::view(&p.config, principal).is_ok())
            .skip(offset as usize)
            .take(limit as usize + 1)
            .collect();
        let more = page.len() > limit as usize;
        page.truncate(limit as usize);
        Ok((page, more))
    }

    pub async fn detail(&self, id: &str, principal: &Principal) -> Result<Publication, Error> {
        let publication = self.catalog.get(id).await.map_err(|e| match e {
            Error::SchemaChanged => Error::DatasetNotFound,
            other => other,
        })?;
        policy::query(&publication.config, principal)?;
        Ok(publication)
    }

    pub async fn get(&self, id: &str) -> Result<Publication, Error> {
        self.catalog.get(id).await
    }

    pub async fn create(&self, body: &[u8]) -> Result<Publication, Error> {
        let dataset: Dataset = decode(body)?;
        self.check(&dataset).await?;
        self.catalog.create(&dataset).await
    }

    pub async fn update(&self, id: &str, body: &[u8], revision: i64) -> Result<Publication, Error> {
        let mut patch: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
            decode(body)?;
        if patch.contains_key("id") {
            return Err(Error::InvalidRequest);
        }
        let current = self.catalog.get(id).await?;
        if current.revision != revision {
            return Err(Error::RevisionMismatch);
        }
        let filter = patch.remove("filter");
        let patch = patch
            .into_iter()
            .map(|(key, raw)| serde_json::from_str(raw.get()).map(|value| (key, value)))
            .collect::<Result<Map<String, Value>, _>>()
            .map_err(|_| Error::InvalidRequest)?;
        let mut config = serde_json::to_value(&current.config).map_err(|_| Error::InternalError)?;
        merge(&mut config, Value::Object(patch));
        let mut dataset: Dataset =
            serde_json::from_value(config).map_err(|_| Error::InvalidRequest)?;
        // Preserve the filter's wire types until the strict predicate parser checks them.
        if let Some(filter) = filter {
            dataset.filter = (filter.get().trim() != "null").then_some(filter);
        }
        self.check(&dataset).await?;
        self.catalog.update(&dataset, revision).await
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
    json!({"id": publication.config.id, "config": publication.config,
        "revision": publication.revision, "metadata": summary(publication)})
}

pub fn summary(publication: &Publication) -> Value {
    let (config, meta) = (&publication.config, &publication.metadata);
    let attributes: Vec<_> = meta
        .attributes
        .iter()
        .map(|a| json!({"name": a.name, "type": a.data_type, "nullable": a.nullable}))
        .collect();
    json!({"id": config.id, "title": config.title, "description": config.description,
        "geometry_type": meta.geometry_type, "srid": meta.srid, "attributes": attributes,
        "formats": ["geojson", "mvt"]})
}

fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    serde_json::from_slice::<Unique>(body).map_err(|_| Error::InvalidRequest)?;
    serde_json::from_slice(body).map_err(|_| Error::InvalidRequest)
}

fn merge(target: &mut Value, patch: Value) {
    let Value::Object(fields) = patch else {
        *target = patch;
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    if let Value::Object(object) = target {
        for (key, value) in fields {
            if value.is_null() {
                object.remove(&key);
            } else {
                merge(object.entry(key).or_insert(Value::Null), value);
            }
        }
    }
}

struct Unique;

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = Unique;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("JSON without duplicate keys")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_str<E>(self, _: &str) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_unit<E>(self) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Unique, A::Error> {
        while seq.next_element::<Unique>()?.is_some() {}
        Ok(Unique)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
        let mut keys = std::collections::HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate key"));
            }
            map.next_value::<Unique>()?;
        }
        Ok(Unique)
    }
}
