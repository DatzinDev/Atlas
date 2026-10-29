use super::{
    predicates::compile_using,
    spatial::{self, Geometry},
};
use crate::{
    config::Limits,
    domain::{Error, Metadata},
    query::predicate::{Predicate, Spatial, parse},
};
use sqlx::{Postgres, QueryBuilder, Transaction};

pub struct Selection {
    predicates: Vec<Predicate>,
    geometries: Vec<Geometry>,
    bbox: Option<Geometry>,
    meta: Metadata,
}

/// Call after authorization, in the same read transaction used to fetch source rows.
pub async fn prepare(
    tx: &mut Transaction<'_, Postgres>,
    fixed: Option<&str>,
    user: Option<&str>,
    bbox: Option<[f64; 4]>,
    meta: &Metadata,
    limits: &Limits,
) -> Result<Selection, Error> {
    limits.validate()?;
    let predicates = [fixed, user]
        .into_iter()
        .flatten()
        .map(|text| parse(text, limits))
        .collect::<Result<Vec<_>, _>>()?;
    // Reject unpublished attributes before even validating geometries in PostGIS.
    for predicate in &predicates {
        compile_using(&mut QueryBuilder::new(""), predicate, meta, &mut |_, _| {
            Ok(())
        })?;
    }
    let mut geometries = Vec::new();
    let mut pending: Vec<_> = predicates.iter().rev().collect();
    while let Some(predicate) = pending.pop() {
        match predicate {
            Predicate::And(args) | Predicate::Or(args) => pending.extend(args.iter().rev()),
            Predicate::Not(arg) => pending.push(arg),
            Predicate::Spatial { polygons, .. } => {
                geometries.push(spatial::prepare(tx, polygons, meta.srid).await?)
            }
            _ => {}
        }
    }
    let bbox = match bbox {
        Some(bounds) => Some(spatial::prepare(tx, &spatial::bbox(bounds)?, meta.srid).await?),
        None => None,
    };
    Ok(Selection {
        predicates,
        geometries,
        bbox,
        meta: meta.clone(),
    })
}

impl Selection {
    pub fn append(&self, builder: &mut QueryBuilder<Postgres>) -> Result<(), Error> {
        let mut geometries = self.geometries.iter();
        builder.push("(");
        for (i, predicate) in self.predicates.iter().enumerate() {
            if i > 0 {
                builder.push(" AND ");
            }
            compile_using(builder, predicate, &self.meta, &mut |builder, op| {
                geometries.next().ok_or(Error::InternalError)?.append(
                    builder,
                    &self.meta.geometry_column,
                    op,
                )
            })?;
        }
        if self.predicates.is_empty() {
            builder.push("TRUE");
        }
        if let Some(bbox) = &self.bbox {
            builder.push(" AND ");
            bbox.append(builder, &self.meta.geometry_column, Spatial::Intersects)?;
        }
        builder.push(")");
        Ok(())
    }
}
