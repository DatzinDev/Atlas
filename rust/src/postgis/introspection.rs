use super::connection::{Database, error};
use crate::{
    config::Config,
    domain::{Attribute, Dataset, Error, Metadata},
    query::predicate::Predicate,
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
    // ponytail: an unparsable fixed filter is reported later by selection::fixed.
    let mut hidden = Vec::new();
    if let Ok(Some(predicate)) = super::selection::fixed(dataset, &config.limits) {
        fields(&predicate, &mut hidden);
    }
    hidden.retain(|field| !dataset.properties.contains(field));
    hidden.sort();
    hidden.dedup();
    let mut tx = database.begin_read().await?;
    let relation = sqlx::query("SELECT c.oid::bigint AS oid, c.relkind::text AS kind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind IN ('r','p','v','m')")
        .bind(&source.schema).bind(&source.table).fetch_optional(&mut *tx).await.map_err(error)?.ok_or(Error::DatasetNotFound)?;
    let oid: i64 = relation.try_get("oid").map_err(error)?;
    let kind: String = relation.try_get("kind").map_err(error)?;
    let rows = sqlx::query("SELECT a.attname AS name, t.typname AS data_type, NOT a.attnotnull AS nullable, CASE WHEN t.typname='geometry' THEN postgis_typmod_type(a.atttypmod) END AS geometry_type, CASE WHEN t.typname='geometry' THEN postgis_typmod_srid(a.atttypmod) END AS srid, EXISTS(SELECT 1 FROM pg_catalog.pg_index i WHERE i.indrelid=a.attrelid AND i.indisprimary AND i.indnkeyatts=1 AND a.attnum=i.indkey[0]) AS primary_key FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type t ON t.oid=a.atttypid WHERE a.attrelid=$1::bigint::oid AND a.attnum>0 AND NOT a.attisdropped AND (t.typnamespace='pg_catalog'::regnamespace AND t.typname::text=ANY($2::text[]) OR (t.typname='geometry' AND t.typnamespace=(SELECT e.extnamespace FROM pg_catalog.pg_extension e WHERE e.extname='postgis'))) ORDER BY a.attnum")
        .bind(oid).bind(crate::domain::PROPERTY_TYPES.iter().map(|t| t.name).collect::<Vec<_>>()).fetch_all(&mut *tx).await.map_err(error)?;
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
    // Missing columns are rejected below; checking first avoids a later permission error (503).
    let mut columns = vec![geometry_column.clone()];
    columns.extend(source.id_column.iter().chain(&primary_key).cloned());
    columns.extend(dataset.properties.iter().chain(&hidden).cloned());
    let readable: bool = sqlx::query_scalar("SELECT coalesce(bool_and(has_column_privilege(current_user, a.attrelid, a.attname::text, 'SELECT')), false) FROM pg_catalog.pg_attribute a WHERE a.attrelid=$1::bigint::oid AND a.attnum>0 AND NOT a.attisdropped AND a.attname::text=ANY($2::text[])")
        .bind(oid).bind(columns).fetch_one(&mut *tx).await.map_err(error)?;
    if !readable {
        return Err(Error::InvalidRequest);
    }
    let relation_name = format!(
        "{}.{}",
        quote_identifier(&source.schema),
        quote_identifier(&source.table)
    );
    if geometry_type.eq_ignore_ascii_case("geometry") || srid <= 0 {
        // A sampled row cannot prove a uniform SRID/type/dimension: scan every non-null row.
        let column = quote_identifier(&geometry_column);
        let mut query = sqlx::QueryBuilder::new("SELECT min(ST_SRID(");
        query
            .push(&column)
            .push(")) AS min_srid, max(ST_SRID(")
            .push(&column)
            .push(")) AS max_srid, max(ST_NDims(")
            .push(&column)
            .push(")) AS dims, array_agg(DISTINCT GeometryType(")
            .push(&column)
            .push(")) AS kinds FROM ")
            .push(&relation_name)
            .push(" WHERE ")
            .push(&column)
            .push(" IS NOT NULL");
        let stats = query.build().fetch_one(&mut *tx).await.map_err(error)?;
        if srid <= 0 {
            let min: Option<i32> = stats.try_get("min_srid").map_err(error)?;
            let max: Option<i32> = stats.try_get("max_srid").map_err(error)?;
            srid = match (min, max) {
                (Some(min), Some(max)) if min == max => min,
                _ => return Err(Error::InvalidRequest),
            };
        }
        if geometry_type.eq_ignore_ascii_case("geometry") {
            let dims: Option<i16> = stats.try_get("dims").map_err(error)?;
            let mut kinds: Vec<String> = stats
                .try_get::<Option<Vec<String>>, _>("kinds")
                .map_err(error)?
                .unwrap_or_default();
            kinds.sort();
            geometry_type = match (dims, kinds.as_slice()) {
                (Some(2), [kind]) => kind.clone(),
                (Some(2), [a, b]) => {
                    let (multi, single) = if a.starts_with("MULTI") {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    // GEOMETRY_TYPES below limits `multi`, and so `single`.
                    if *multi != format!("MULTI{single}") {
                        return Err(Error::InvalidRequest);
                    }
                    multi.clone()
                }
                _ => return Err(Error::InvalidRequest),
            };
        }
    }
    geometry_type.make_ascii_uppercase();
    if srid <= 0 || !crate::domain::GEOMETRY_TYPES.contains(&geometry_type.as_str()) {
        return Err(Error::InvalidRequest);
    }
    let known_crs: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM spatial_ref_sys WHERE srid=$1)")
            .bind(srid)
            .fetch_one(&mut *tx)
            .await
            .map_err(error)?;
    if !known_crs {
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
    if !crate::domain::ID_TYPES.contains(&id.data_type.as_str()) {
        return Err(Error::InvalidRequest);
    }
    if ["v", "m"].contains(&kind.as_str()) {
        // A plain valid unique index on id_column of a materialized view proves uniqueness.
        let unique = kind == "m"
            && sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_index i JOIN pg_catalog.pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=i.indkey[0] WHERE i.indrelid=$1::bigint::oid AND i.indisunique AND i.indisvalid AND i.indpred IS NULL AND i.indexprs IS NULL AND i.indnkeyatts=1 AND a.attname::text=$2)")
                .bind(oid)
                .bind(&id_column)
                .fetch_one(&mut *tx)
                .await
                .map_err(error)?;
        let id = quote_identifier(&id_column);
        let mut query = sqlx::QueryBuilder::new("SELECT ");
        if unique {
            query
                .push("NOT EXISTS(SELECT 1 FROM ")
                .push(&relation_name)
                .push(" WHERE ")
                .push(&id)
                .push(" IS NULL)");
        } else {
            query
                .push("count(*)=count(")
                .push(&id)
                .push(") AND count(*)=count(DISTINCT ")
                .push(&id)
                .push(") FROM ")
                .push(&relation_name);
        }
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
        // ponytail: the introspection query only returns columns of supported base types.
        let column = attributes
            .iter()
            .find(|a| &a.name == property)
            .ok_or(Error::InvalidRequest)?;
        published.push(column.clone());
    }
    // Missing, geometry or unsupported-type columns are absent from `attributes`.
    let filter_attributes = hidden
        .iter()
        .map(|field| {
            attributes
                .iter()
                .find(|a| &a.name == field)
                .cloned()
                .ok_or(Error::InvalidRequest)
        })
        .collect::<Result<Vec<_>, _>>()?;
    // Densify the box before ST_Transform so curved edges of projected CRSs stay inside.
    let mut query = sqlx::QueryBuilder::new(
        "SELECT least(greatest(ST_XMin(e),-180),180)::float8 AS minx, least(greatest(ST_YMin(e),-90),90)::float8 AS miny, least(greatest(ST_XMax(e),-180),180)::float8 AS maxx, least(greatest(ST_YMax(e),-90),90)::float8 AS maxy FROM (SELECT ST_Transform(ST_Segmentize(ST_SetSRID(b::geometry, ",
    );
    query.push_bind(srid).push(
        "), greatest(ST_XMax(b)-ST_XMin(b), ST_YMax(b)-ST_YMin(b), 1e-9)/32), 4326) AS e FROM (SELECT coalesce(",
    );
    // ponytail: ST_EstimatedExtent rejects relation names containing '"', so ST_Extent is used for those.
    if kind == "v" || source.schema.contains('"') || source.table.contains('"') {
        query.push("NULL::box2d");
    } else {
        query
            .push("ST_EstimatedExtent(")
            .push_bind(&source.schema)
            .push(", ")
            .push_bind(&source.table)
            .push(", ")
            .push_bind(geometry_column.clone())
            .push(")");
    }
    query
        .push(", (SELECT ST_Extent(")
        .push(quote_identifier(&geometry_column))
        .push(") FROM ")
        .push(&relation_name)
        .push(")) AS b) s WHERE b IS NOT NULL) t");
    let extent: Option<[f64; 4]> = match query
        .build()
        .fetch_optional(&mut *tx)
        .await
        .map_err(error)?
    {
        Some(row) => Some([
            row.try_get("minx").map_err(error)?,
            row.try_get("miny").map_err(error)?,
            row.try_get("maxx").map_err(error)?,
            row.try_get("maxy").map_err(error)?,
        ]),
        None => None,
    };
    tx.rollback().await.map_err(error)?;
    Ok(Metadata {
        geometry_column,
        geometry_type,
        srid,
        id_column,
        id_type: id.data_type.clone(),
        attributes: published,
        filter_attributes,
        extent,
    })
}

fn fields(predicate: &Predicate, out: &mut Vec<String>) {
    match predicate {
        Predicate::Compare { field, .. }
        | Predicate::In { field, .. }
        | Predicate::Null { field, .. } => out.push(field.clone()),
        Predicate::And(args) | Predicate::Or(args) => args.iter().for_each(|arg| fields(arg, out)),
        Predicate::Not(arg) => fields(arg, out),
        Predicate::Spatial { .. } => {}
    }
}
