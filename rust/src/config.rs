use crate::domain::{Error, names};
use serde::Deserialize;
use std::{
    collections::HashSet,
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Auth {
    pub public_key_file: PathBuf,
    pub issuer: String,
    pub audience: String,
    #[serde(default = "default_leeway")]
    pub leeway_seconds: u64,
}

fn default_leeway() -> u64 {
    crate::auth::jwt::DEFAULT_LEEWAY_SECONDS
}

impl Auth {
    pub fn validate(&self) -> Result<(), Error> {
        if self.public_key_file.as_os_str().is_empty()
            || self.issuer.trim().is_empty()
            || self.audience.trim().is_empty()
            || self.leeway_seconds > 300
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }

    pub fn verifier(&self) -> Result<crate::auth::jwt::Verifier, Error> {
        self.validate()?;
        let pem = std::fs::read(&self.public_key_file).map_err(|_| Error::InvalidRequest)?;
        crate::auth::jwt::Verifier::new(&pem, &self.issuer, &self.audience)
            .map(|v| v.with_leeway(self.leeway_seconds))
    }
}

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

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cors {
    /// Empty (default) disables CORS; `["*"]` allows any origin.
    pub allowed_origins: Vec<String>,
}

impl Cors {
    pub fn validate(&self) -> Result<(), Error> {
        let origins = &self.allowed_origins;
        // ponytail: character whitelist plus numeric port, not a full host parser.
        let origin = |o: &String| {
            let Some(rest) = o.strip_prefix("https://").or(o.strip_prefix("http://")) else {
                return false;
            };
            let split = rest.rsplit_once(':').filter(|(_, p)| !p.contains(']'));
            let (host, port) = split.unwrap_or((rest, "1"));
            let v6 = host.starts_with('[') && host.ends_with(']');
            let h = if v6 { &host[1..host.len() - 1] } else { host };
            let ok = |c: char| c.is_ascii_alphanumeric() || "-.".contains(c) || (v6 && c == ':');
            !h.is_empty()
                && h.chars().all(ok)
                && !port.is_empty()
                && port.bytes().all(|b| b.is_ascii_digit())
        };
        let unique = origins.iter().collect::<HashSet<_>>().len() == origins.len();
        if origins.len() > 100 || !unique || !(*origins == ["*"] || origins.iter().all(origin)) {
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
    pub auth: Option<Auth>,
    pub allowed_schemas: Vec<String>,
    pub public_url: Option<String>,
    pub limits: Limits,
    pub cors: Cors,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: ([127, 0, 0, 1], 8080).into(),
            database_url: None,
            auth: None,
            allowed_schemas: vec!["public".into()],
            public_url: None,
            limits: Limits::default(),
            cors: Cors::default(),
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
        self.cors.validate()?;
        if let Some(auth) = &self.auth {
            auth.validate()?;
        }
        if !names(&self.allowed_schemas)
            || self.allowed_schemas.iter().any(|s| {
                ["atlas_catalog", "information_schema"].contains(&s.as_str())
                    || s.starts_with("pg_")
            })
            || self.database_url.as_ref().is_some_and(|s| {
                !(s.starts_with("postgres://") || s.starts_with("postgresql://"))
                    || s.chars().any(char::is_control)
            })
            || self.public_url.as_ref().is_some_and(|s| {
                !(s.starts_with("https://") || s.starts_with("http://"))
                    || s.ends_with('/')
                    || s.contains(['?', '#'])
                    || s.chars().any(|c| c.is_control() || c.is_whitespace())
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
