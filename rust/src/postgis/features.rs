use super::{introspection::quote_identifier, predicates::query_error, selection::Selection};
use crate::{
    config::Limits,
    domain::{Dataset, Direction, Error, Metadata, Query},
};
use serde_json::value::RawValue;
use sqlx::{Postgres, QueryBuilder, Transaction};

pub struct Page {
    pub features: Vec<Box<RawValue>>,
    pub more: bool,
}

pub async fn page(
    tx: &mut Transaction<'_, Postgres>,
    dataset: &Dataset,
    meta: &Metadata,
    selection: &Selection,
    query: &Query,
    limits: &Limits,
) -> Result<Page, Error> {
    query.validate(limits.max_limit, limits.max_offset)?;
    let published: Vec<&str> = meta.attributes.iter().map(|a| a.name.as_str()).collect();
    let properties = match &query.properties {
        Some(names) => names.iter().map(String::as_str).collect(),
        None => published.clone(),
    };
    if properties.iter().any(|name| !published.contains(name)) {
        return Err(Error::InvalidRequest);
    }
    let limit = query.limit.unwrap_or(limits.default_limit);
    let geometry = quote_identifier(&meta.geometry_column);
    let mut builder =
        QueryBuilder::<Postgres>::new("SELECT json_build_object('type', 'Feature', 'id', ");
    builder
        .push(quote_identifier(&meta.id_column))
        .push(", 'geometry', ST_AsGeoJSON(");
    if meta.srid == 4326 {
        builder.push(&geometry);
    } else {
        builder
            .push("ST_Transform(")
            .push(&geometry)
            .push(", 4326)");
    }
    builder.push(")::json, 'properties', json_build_object(");
    for (i, name) in properties.iter().enumerate() {
        if i > 0 {
            builder.push(", ");
        }
        builder
            .push_bind(name.to_string())
            .push("::text, ")
            .push(quote_identifier(name));
    }
    builder
        .push("))::text FROM ")
        .push(quote_identifier(&dataset.source.schema))
        .push(".")
        .push(quote_identifier(&dataset.source.table))
        .push(" WHERE ");
    selection.append(&mut builder)?;
    let mut order = Vec::new();
    for field in query.sort.iter().flatten() {
        if field.field != meta.id_column && !published.contains(&field.field.as_str()) {
            return Err(Error::InvalidRequest);
        }
        let direction = match field.direction {
            Direction::Asc => "ASC",
            Direction::Desc => "DESC",
        };
        order.push((field.field.as_str(), direction));
    }
    if !order.iter().any(|(name, _)| *name == meta.id_column) {
        order.push((&meta.id_column, "ASC"));
    }
    let order: Vec<_> = order
        .into_iter()
        .map(|(name, direction)| format!("{} {direction}", quote_identifier(name)))
        .collect();
    builder.push(" ORDER BY ").push(order.join(", "));
    builder
        .push(" LIMIT ")
        .push_bind(i64::from(limit) + 1)
        .push(" OFFSET ")
        .push_bind(i64::from(query.offset));
    let rows: Vec<String> = builder
        .build_query_scalar()
        .fetch_all(&mut **tx)
        .await
        .map_err(query_error)?;
    let more = rows.len() > limit as usize;
    let features = rows
        .into_iter()
        .take(limit as usize)
        .map(|row| RawValue::from_string(row).map_err(|_| Error::InternalError))
        .collect::<Result<_, _>>()?;
    Ok(Page { features, more })
}
