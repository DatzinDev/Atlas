use super::connection::{Database, error};
use crate::{
    config::Config,
    domain::{Attribute, Dataset, Error, Metadata},
};
use sqlx::Row;

pub fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub async fn inspect(
    database: &Database,
    config: &Config,
    dataset: &Dataset,
) -> Result<Metadata, Error> {
    dataset.validate(&config.allowed_schemas)?;
    let source = &dataset.source;
    let mut tx = database.begin_read().await?;
    let relation = sqlx::query("SELECT c.oid::bigint AS oid, c.relkind::text AS kind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind IN ('r','p','v','m')")
        .bind(&source.schema).bind(&source.table).fetch_optional(&mut *tx).await.map_err(error)?.ok_or(Error::DatasetNotFound)?;
    let oid: i64 = relation.try_get("oid").map_err(error)?;
    let kind: String = relation.try_get("kind").map_err(error)?;
    let rows = sqlx::query("SELECT a.attname AS name, t.typname AS data_type, NOT a.attnotnull AS nullable, CASE WHEN t.typname='geometry' THEN postgis_typmod_type(a.atttypmod) END AS geometry_type, CASE WHEN t.typname='geometry' THEN postgis_typmod_srid(a.atttypmod) END AS srid, EXISTS(SELECT 1 FROM pg_catalog.pg_index i WHERE i.indrelid=a.attrelid AND i.indisprimary AND i.indnkeyatts=1 AND a.attnum=i.indkey[0]) AS primary_key FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type t ON t.oid=a.atttypid WHERE a.attrelid=$1::bigint::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum")
        .bind(oid).fetch_all(&mut *tx).await.map_err(error)?;
    let mut attributes = Vec::new();
    let (mut geometries, mut primary_key) = (Vec::new(), None);
    for row in rows {
        let name: String = row.try_get("name").map_err(error)?;
        let data_type: String = row.try_get("data_type").map_err(error)?;
        if data_type == "geometry" {
            geometries.push((
                name,
                row.try_get::<String, _>("geometry_type").map_err(error)?,
                row.try_get::<i32, _>("srid").map_err(error)?,
            ));
        } else {
            if row.try_get::<bool, _>("primary_key").map_err(error)? {
                primary_key = Some(name.clone());
            }
            attributes.push(Attribute {
                name,
                data_type,
                nullable: row.try_get("nullable").map_err(error)?,
            });
        }
    }
    let geometry = match source.geometry_column.as_ref() {
        Some(name) => geometries.iter().find(|g| &g.0 == name),
        None if geometries.len() == 1 => geometries.first(),
        _ => None,
    }
    .ok_or(Error::InvalidRequest)?;
    let (geometry_column, mut geometry_type, mut srid) = geometry.clone();
    let relation_name = format!(
        "{}.{}",
        quote_identifier(&source.schema),
        quote_identifier(&source.table)
    );
    if geometry_type.eq_ignore_ascii_case("geometry") || srid <= 0 {
        let column = quote_identifier(&geometry_column);
        let mut query = sqlx::QueryBuilder::new("SELECT GeometryType(");
        query
            .push(&column)
            .push(") AS kind, ST_SRID(")
            .push(&column)
            .push(") AS srid FROM ")
            .push(&relation_name)
            .push(" WHERE ")
            .push(&column)
            .push(" IS NOT NULL LIMIT 1");
        let sample = query
            .build()
            .fetch_optional(&mut *tx)
            .await
            .map_err(error)?
            .ok_or(Error::InvalidRequest)?;
        geometry_type = sample.try_get("kind").map_err(error)?;
        srid = sample.try_get("srid").map_err(error)?;
    }
    geometry_type.make_ascii_uppercase();
    if srid <= 0
        || ![
            "POINT",
            "MULTIPOINT",
            "LINESTRING",
            "MULTILINESTRING",
            "POLYGON",
            "MULTIPOLYGON",
            "GEOMETRYCOLLECTION",
        ]
        .contains(&geometry_type.as_str())
    {
        return Err(Error::InvalidRequest);
    }
    let id_column = if ["v", "m"].contains(&kind.as_str()) {
        source.id_column.clone().ok_or(Error::InvalidRequest)?
    } else {
        let pk = primary_key.ok_or(Error::InvalidRequest)?;
        if source.id_column.as_ref().is_some_and(|id| id != &pk) {
            return Err(Error::InvalidRequest);
        }
        pk
    };
    let id = attributes
        .iter()
        .find(|a| a.name == id_column)
        .ok_or(Error::InvalidRequest)?;
    if !["int2", "int4", "int8", "text", "varchar", "bpchar"].contains(&id.data_type.as_str()) {
        return Err(Error::InvalidRequest);
    }
    if ["v", "m"].contains(&kind.as_str()) {
        let id = quote_identifier(&id_column);
        let mut query = sqlx::QueryBuilder::new("SELECT count(*)=count(");
        query
            .push(&id)
            .push(") AND count(*)=count(DISTINCT ")
            .push(&id)
            .push(") FROM ")
            .push(&relation_name);
        if !query
            .build_query_scalar::<bool>()
            .fetch_one(&mut *tx)
            .await
            .map_err(error)?
        {
            return Err(Error::InvalidRequest);
        }
    }
    let mut published = Vec::new();
    for property in &dataset.properties {
        let column = attributes
            .iter()
            .find(|a| &a.name == property)
            .ok_or(Error::InvalidRequest)?;
        if ![
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
        .contains(&column.data_type.as_str())
        {
            return Err(Error::InvalidRequest);
        }
        published.push(column.clone());
    }
    tx.rollback().await.map_err(error)?;
    Ok(Metadata {
        geometry_column,
        geometry_type,
        srid,
        id_column,
        attributes: published,
    })
}
