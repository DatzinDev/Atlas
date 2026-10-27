use crate::domain::{Error, names};
use serde::Deserialize;
use std::{net::SocketAddr, path::Path};

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub default_limit: u32,
    pub max_limit: u32,
    pub max_offset: u32,
    pub catalog_default: u32,
    pub catalog_max: u32,
    pub filter_bytes: u32,
    pub filter_depth: u32,
    pub filter_nodes: u32,
    pub in_values: u32,
    pub xy_pairs: u32,
    pub pool_min: u32,
    pub pool_max: u32,
    pub acquire_seconds: u32,
    pub query_seconds: u32,
    pub stream_seconds: u32,
    pub data_operations: u32,
    pub tile_candidates: u32,
    pub tile_bytes: u32,
    pub tile_zoom: u32,
    pub tile_extent: u32,
    pub tile_buffer: u32,
    pub export_features: u32,
    pub export_bytes: u32,
    pub query_body_bytes: u32,
    pub admin_body_bytes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            default_limit: 1000,
            max_limit: 10000,
            max_offset: 100000,
            catalog_default: 100,
            catalog_max: 1000,
            filter_bytes: 65536,
            filter_depth: 8,
            filter_nodes: 100,
            in_values: 1000,
            xy_pairs: 10000,
            pool_min: 1,
            pool_max: 10,
            acquire_seconds: 5,
            query_seconds: 30,
            stream_seconds: 300,
            data_operations: 8,
            tile_candidates: 100000,
            tile_bytes: 8 * 1024 * 1024,
            tile_zoom: 22,
            tile_extent: 4096,
            tile_buffer: 64,
            export_features: 1000000,
            export_bytes: 256 * 1024 * 1024,
            query_body_bytes: 128 * 1024,
            admin_body_bytes: 256 * 1024,
        }
    }
}

impl Limits {
    pub fn validate(&self) -> Result<(), Error> {
        let positive = [
            self.default_limit,
            self.max_limit,
            self.catalog_default,
            self.catalog_max,
            self.filter_bytes,
            self.filter_depth,
            self.filter_nodes,
            self.in_values,
            self.xy_pairs,
            self.pool_max,
            self.acquire_seconds,
            self.query_seconds,
            self.stream_seconds,
            self.data_operations,
            self.tile_candidates,
            self.tile_bytes,
            self.tile_extent,
            self.export_features,
            self.export_bytes,
            self.query_body_bytes,
            self.admin_body_bytes,
        ];
        if positive.contains(&0)
            || self.default_limit > self.max_limit
            || self.catalog_default > self.catalog_max
            || self.pool_min > self.pool_max
            || self.pool_max > 10
            || self.tile_zoom > 22
            || self.tile_extent > 65536
            || self.tile_buffer > self.tile_extent
            || self.query_seconds > i32::MAX as u32 / 1000
            || self.filter_depth > 128
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    pub database_url: Option<String>,
    pub allowed_schemas: Vec<String>,
    pub limits: Limits,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: ([127, 0, 0, 1], 8080).into(),
            database_url: None,
            allowed_schemas: vec!["public".into()],
            limits: Limits::default(),
        }
    }
}

impl Config {
    pub fn parse(
        text: &str,
        database_url: Option<String>,
        listen: Option<String>,
    ) -> Result<Self, Error> {
        let mut config: Self = toml::from_str(text).map_err(|_| Error::InvalidRequest)?;
        if database_url.is_some() {
            config.database_url = database_url;
        }
        if let Some(listen) = listen {
            config.listen = listen.parse().map_err(|_| Error::InvalidRequest)?;
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), Error> {
        self.limits.validate()?;
        if !names(&self.allowed_schemas)
            || self.allowed_schemas.iter().any(|s| s == "atlas_catalog")
            || self.database_url.as_ref().is_some_and(|s| {
                !(s.starts_with("postgres://") || s.starts_with("postgresql://"))
                    || s.chars().any(char::is_control)
            })
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }

    pub fn load(path: Option<&Path>, listen: Option<String>) -> Result<Self, Error> {
        let text = path
            .map(std::fs::read_to_string)
            .transpose()
            .map_err(|_| Error::InvalidRequest)?
            .unwrap_or_default();
        let database =
            std::env::var("ATLAS_DATABASE_URL")
                .map(Some)
                .or_else(|error| match error {
                    std::env::VarError::NotPresent => Ok(None),
                    _ => Err(Error::InvalidRequest),
                })?;
        let listen = listen.or(std::env::var("ATLAS_LISTEN").ok());
        Self::parse(&text, database, listen)
    }
}
