use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::value::RawValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    InvalidRequest,
    InvalidFilter,
    InvalidToken,
    PermissionDenied,
    DatasetNotFound,
    NotFound,
    MethodNotAllowed,
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

#[derive(Clone, Default, Deserialize)]
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

/// WGS84 bbox `[minx, miny, maxx, maxy]` with positive area; NaN fails every comparison.
pub fn bbox(b: [f64; 4]) -> bool {
    let [x0, y0, x1, y1] = b;
    -180.0 <= x0 && x0 < x1 && x1 <= 180.0 && -90.0 <= y0 && y0 < y1 && y1 <= 90.0
}

pub const MAX_ROLES: usize = 1000;
pub const MAX_ROLE_LENGTH: usize = 128;
pub const ID_TYPES: [&str; 6] = ["int2", "int4", "int8", "text", "varchar", "bpchar"];
pub const GEOMETRY_TYPES: [&str; 7] = [
    "POINT",
    "MULTIPOINT",
    "LINESTRING",
    "MULTILINESTRING",
    "POLYGON",
    "MULTIPOLYGON",
    "GEOMETRYCOLLECTION",
];

/// How a filter literal is validated before it is bound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FilterClass {
    Text,
    Date,
    Timestamp { zoned: bool },
    Integer(i64, i64),
    Float(f64),
    Numeric,
    Bool,
}

/// How a property is selected into an MVT tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MvtEncoding {
    Native,
    Text,
    IsoText,
    FiniteFloat,
}

#[derive(Debug)]
pub struct PropertyType {
    pub name: &'static str,
    pub filter: FilterClass,
    pub mvt: MvtEncoding,
    pub tilejson: &'static str,
}

const fn property(
    name: &'static str,
    filter: FilterClass,
    mvt: MvtEncoding,
    tilejson: &'static str,
) -> PropertyType {
    PropertyType {
        name,
        filter,
        mvt,
        tilejson,
    }
}

/// The only list of publishable PostgreSQL property types (pg_catalog typname).
pub static PROPERTY_TYPES: [PropertyType; 13] = {
    use FilterClass as F;
    use MvtEncoding as M;
    [
        property(
            "int2",
            F::Integer(i16::MIN as i64, i16::MAX as i64),
            M::Native,
            "Number",
        ),
        property(
            "int4",
            F::Integer(i32::MIN as i64, i32::MAX as i64),
            M::Native,
            "Number",
        ),
        property("int8", F::Integer(i64::MIN, i64::MAX), M::Native, "Number"),
        property(
            "float4",
            F::Float(f32::MAX as f64),
            M::FiniteFloat,
            "Number",
        ),
        property("float8", F::Float(f64::MAX), M::FiniteFloat, "Number"),
        property("numeric", F::Numeric, M::Text, "String"),
        property("bool", F::Bool, M::Native, "Boolean"),
        property("text", F::Text, M::Native, "String"),
        property("varchar", F::Text, M::Native, "String"),
        property("bpchar", F::Text, M::Native, "String"),
        property("date", F::Date, M::IsoText, "String"),
        property(
            "timestamp",
            F::Timestamp { zoned: false },
            M::IsoText,
            "String",
        ),
        property(
            "timestamptz",
            F::Timestamp { zoned: true },
            M::IsoText,
            "String",
        ),
    ]
};

pub fn property_type(name: &str) -> Option<&'static PropertyType> {
    PROPERTY_TYPES.iter().find(|t| t.name == name)
}

pub fn roles(values: &[String]) -> bool {
    values.len() <= MAX_ROLES
        && values.iter().all(|r| {
            (1..=MAX_ROLE_LENGTH).contains(&r.chars().count())
                && !r
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '\u{FEFF}')
        })
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
            .any(|list| !roles(list))
        {
            return Err(Error::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    #[serde(default, deserialize_with = "filter")]
    pub filter: Option<Box<RawValue>>,
    #[serde(default, deserialize_with = "present")]
    pub bbox: Option<[f64; 4]>,
    #[serde(default, deserialize_with = "present")]
    pub properties: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    pub sort: Option<Vec<SortField>>,
    #[serde(default, deserialize_with = "present")]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: u32,
}

// An absent field is None; an explicit JSON null is rejected.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

fn filter<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Box<RawValue>>, D::Error> {
    let raw = Box::<RawValue>::deserialize(d)?;
    if raw.get().trim() == "null" {
        return Err(de::Error::custom("null filter"));
    }
    Ok(Some(raw))
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SortField {
    pub field: String,
    pub direction: Direction,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Asc,
    Desc,
}

impl Query {
    pub fn validate(&self, max_limit: u32, max_offset: u32) -> Result<(), Error> {
        if self.limit.is_some_and(|n| n == 0 || n > max_limit)
            || self.offset > max_offset
            || self.bbox.is_some_and(|b| !bbox(b))
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
    pub id_type: String,
    pub attributes: Vec<Attribute>,
    /// Unpublished columns referenced only by the fixed filter; never exposed.
    #[serde(default)]
    pub filter_attributes: Vec<Attribute>,
    /// EPSG:4326 `[minx, miny, maxx, maxy]` captured at publish time; `None` without rows.
    #[serde(default)]
    pub extent: Option<[f64; 4]>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attribute {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}
