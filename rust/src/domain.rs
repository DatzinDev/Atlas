use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    InvalidRequest,
    InvalidFilter,
    InvalidToken,
    PermissionDenied,
    DatasetNotFound,
    PublicationConflict,
    SchemaChanged,
    RevisionMismatch,
    QueryTooLarge,
    InternalError,
    CapacityExceeded,
    DatabaseUnavailable,
    QueryTimeout,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    pub sub: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AccessPolicy {
    pub public: bool,
    pub public_download: bool,
    pub view_roles: Vec<String>,
    pub query_roles: Vec<String>,
    pub download_roles: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub schema: String,
    pub table: String,
    pub geometry_column: Option<String>,
    pub id_column: Option<String>,
}

pub fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 63 && !value.contains('\0')
}

pub fn names(values: &[String]) -> bool {
    let unique: std::collections::HashSet<_> = values.iter().collect();
    !values.is_empty() && unique.len() == values.len() && values.iter().all(|v| identifier(v))
}

impl Source {
    pub fn validate(&self, allowed: &[String]) -> Result<(), Error> {
        if !allowed.contains(&self.schema)
            || ![
                Some(&self.schema),
                Some(&self.table),
                self.geometry_column.as_ref(),
                self.id_column.as_ref(),
            ]
            .into_iter()
            .flatten()
            .all(|s| identifier(s))
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub source: Source,
    pub properties: Vec<String>,
    pub filter: Option<Box<RawValue>>,
    #[serde(default)]
    pub access: AccessPolicy,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn enabled() -> bool {
    true
}

impl Dataset {
    pub fn validate(&self, allowed: &[String]) -> Result<(), Error> {
        self.source.validate(allowed)?;
        if self.id.is_empty()
            || self.id.len() > 128
            || !self
                .id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
            || self.title.trim().is_empty()
            || !names(&self.properties)
            || (self.access.public_download && !self.access.public)
            || [
                &self.access.view_roles,
                &self.access.query_roles,
                &self.access.download_roles,
            ]
            .into_iter()
            .flatten()
            .any(|r| r.trim().is_empty())
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub filter: Option<Box<RawValue>>,
    pub bbox: Option<[f64; 4]>,
    pub properties: Option<Vec<String>>,
    pub sort: Option<Vec<SortField>>,
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SortField {
    pub field: String,
    pub direction: Direction,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Asc,
    Desc,
}

impl Query {
    pub fn validate(&self, max_limit: u32, max_offset: u32) -> Result<(), Error> {
        if self.limit.is_some_and(|n| n == 0 || n > max_limit)
            || self.offset > max_offset
            || self
                .bbox
                .is_some_and(|b| b.iter().any(|v| !v.is_finite()) || b[0] > b[2] || b[1] > b[3])
            || self.properties.as_ref().is_some_and(|v| !names(v))
            || self
                .sort
                .as_ref()
                .is_some_and(|v| !names(&v.iter().map(|s| s.field.clone()).collect::<Vec<_>>()))
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub geometry_column: String,
    pub geometry_type: String,
    pub srid: i32,
    pub id_column: String,
    pub attributes: Vec<Attribute>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attribute {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}
