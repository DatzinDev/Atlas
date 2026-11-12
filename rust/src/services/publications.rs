use crate::{
    auth::policy,
    catalog::{Catalog, Publication},
    config::{Config, Limits},
    domain::{Dataset, Error, Principal, Query},
    postgis::{
        connection::Database,
        features::{self, Page},
        introspection::inspect,
        mvt,
        selection::prepare,
    },
};
use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeOwned, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value, json};

/// Percent-encodes every byte except RFC 3986 unreserved characters.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Clone)]
pub struct Publications {
    catalog: Catalog,
    database: Database,
    config: Config,
    admission: super::admission::Admission,
    exports: super::admission::Admission,
}

impl Publications {
    pub fn new(database: Database, config: Config) -> Result<Self, Error> {
        Ok(Self {
            catalog: Catalog::new(database.clone(), config.clone())?,
            database,
            admission: super::admission::Admission::new(
                config.limits.data_operations,
                config.limits.query_seconds,
            ),
            exports: super::admission::Admission::new(
                config.limits.export_operations,
                config.limits.query_seconds,
            ),
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
        let mut page = self
            .catalog
            .servable(
                |p| policy::view(&p.config, principal).is_ok(),
                offset as usize,
                limit as usize + 1,
            )
            .await?;
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

    pub async fn tile(
        &self,
        publication: &Publication,
        xyz: (u32, u32, u32),
        filter: Option<&str>,
        properties: Option<&[String]>,
    ) -> Result<Vec<u8>, Error> {
        let permit = self.admission.acquire()?;
        let (service, publication, filter, properties) = (
            self.clone(),
            publication.clone(),
            filter.map(str::to_owned),
            properties.map(<[String]>::to_vec),
        );
        self.admission
            .wait(async move {
                let _permit = permit;
                let mut tx = service.database.begin_data().await?;
                let result = async {
                    let selection = prepare(
                        &mut tx,
                        publication.config.filter.as_ref().map(|f| f.get()),
                        filter.as_deref(),
                        None,
                        &publication.metadata,
                        &service.config.limits,
                    )
                    .await?;
                    mvt::tile(
                        &mut tx,
                        &publication.config,
                        &publication.metadata,
                        &selection,
                        xyz,
                        properties.as_deref(),
                        &service.config.limits,
                    )
                    .await
                }
                .await;
                let cleanup = tx
                    .rollback()
                    .await
                    .map_err(crate::postgis::connection::error);
                cleanup.and(result)
            })
            .await
    }

    pub async fn features(&self, publication: &Publication, query: &Query) -> Result<Page, Error> {
        let permit = self.admission.acquire()?;
        let (service, publication, query) = (self.clone(), publication.clone(), query.clone());
        self.admission
            .wait(async move {
                let _permit = permit;
                let mut tx = service.database.begin_data().await?;
                let result = async {
                    let selection = prepare(
                        &mut tx,
                        publication.config.filter.as_ref().map(|f| f.get()),
                        query.filter.as_ref().map(|f| f.get()),
                        query.bbox,
                        &publication.metadata,
                        &service.config.limits,
                    )
                    .await?;
                    features::page(
                        &mut tx,
                        &publication.config,
                        &publication.metadata,
                        &selection,
                        &query,
                        &service.config.limits,
                    )
                    .await
                }
                .await;
                let cleanup = tx
                    .rollback()
                    .await
                    .map_err(crate::postgis::connection::error);
                cleanup.and(result)
            })
            .await
    }

    pub async fn export(
        &self,
        id: &str,
        principal: &Principal,
        query: &Query,
    ) -> Result<crate::exports::geojson::Export, Error> {
        query.validate(self.config.limits.max_limit, self.config.limits.max_offset)?;
        if let Some(filter) = &query.filter {
            crate::query::predicate::parse(filter.get(), &self.config.limits)?;
        }
        let publication = self.catalog.get(id).await.map_err(|e| match e {
            Error::SchemaChanged => Error::DatasetNotFound,
            other => other,
        })?;
        policy::download(&publication.config, principal)?;
        // Exports hold a sub-slot for their whole stream so items and tiles keep the rest.
        let permits = vec![self.exports.acquire()?, self.admission.acquire()?];
        let (service, query) = (self.clone(), query.clone());
        self.admission
            .wait(async move {
                crate::exports::geojson::Export::admitted(
                    &service.database,
                    &publication,
                    &query,
                    &service.config.limits,
                    permits,
                )
                .await
            })
            .await
    }

    pub async fn tilejson(
        &self,
        publication: &Publication,
        filter: Option<&str>,
        properties: Option<&[String]>,
    ) -> Result<Value, Error> {
        let (config, meta) = (&publication.config, &publication.metadata);
        if let Some(filter) = filter {
            let permit = self.admission.acquire()?;
            let (service, publication, filter) =
                (self.clone(), publication.clone(), filter.to_owned());
            self.admission
                .wait(async move {
                    let _permit = permit;
                    let mut tx = service.database.begin_read().await?;
                    let result = prepare(
                        &mut tx,
                        publication.config.filter.as_ref().map(|f| f.get()),
                        Some(filter.as_str()),
                        None,
                        &publication.metadata,
                        &service.config.limits,
                    )
                    .await
                    .map(drop);
                    let cleanup = tx
                        .rollback()
                        .await
                        .map_err(crate::postgis::connection::error);
                    cleanup.and(result)
                })
                .await?;
        }
        if let Some(names) = properties {
            let unique: std::collections::HashSet<_> = names.iter().collect();
            let published = |n: &String| meta.attributes.iter().any(|a| a.name == *n);
            if unique.len() != names.len() || !names.iter().all(published) {
                return Err(Error::InvalidRequest);
            }
        }
        let fields: Map<String, Value> = meta
            .attributes
            .iter()
            .filter(|a| properties.is_none_or(|names| names.contains(&a.name)))
            .map(|a| {
                let kind = match a.data_type.as_str() {
                    "int2" | "int4" | "int8" | "float4" | "float8" => "Number",
                    "bool" => "Boolean",
                    _ => "String",
                };
                (a.name.clone(), json!(kind))
            })
            .collect();
        let base = self.config.public_url.as_deref().unwrap_or_default();
        let mut url = format!(
            "{base}/api/v1/datasets/{}/tiles/{{z}}/{{x}}/{{y}}.mvt",
            config.id
        );
        let query: Vec<String> = [
            ("filter", filter.map(str::to_owned)),
            ("properties", properties.map(|names| names.join(","))),
        ]
        .into_iter()
        .filter_map(|(key, value)| Some(format!("{key}={}", encode(&value?))))
        .collect();
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query.join("&"));
        }
        Ok(
            json!({"tilejson": "3.0.0", "name": config.title, "tiles": [url], "minzoom": 0,
                "maxzoom": self.config.limits.tile_zoom,
                "vector_layers": [{"id": config.id, "fields": fields}]}),
        )
    }

    pub fn limits(&self) -> &Limits {
        &self.config.limits
    }

    pub async fn get(&self, id: &str) -> Result<Publication, Error> {
        self.catalog.get(id).await
    }

    pub async fn create(&self, body: &[u8]) -> Result<Publication, Error> {
        let fields: Map<String, Value> = decode(body)?;
        // The fixed filter keeps its own grammar; only a null filter itself is rejected here.
        if fields.iter().any(|(key, value)| match key.as_str() {
            "filter" => value.is_null(),
            _ => has_null(value),
        }) {
            return Err(Error::InvalidRequest);
        }
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
        "geometry_type": meta.geometry_type, "srid": meta.srid, "extent": meta.extent, "attributes": attributes,
        "formats": ["geojson", "mvt"]})
}

fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    serde_json::from_slice::<Unique>(body).map_err(|_| Error::InvalidRequest)?;
    serde_json::from_slice(body).map_err(|_| Error::InvalidRequest)
}

fn has_null(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.iter().any(has_null),
        Value::Object(fields) => fields.values().any(has_null),
        _ => false,
    }
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
