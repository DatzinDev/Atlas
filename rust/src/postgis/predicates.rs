use super::{connection::error, introspection::quote_identifier};
use crate::{
    domain::{Error, Metadata},
    query::predicate::{Comparison, Predicate, Scalar},
};
use sqlx::{Postgres, QueryBuilder};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Temporal,
    Integer(i64, i64),
    Float(f64),
    Numeric,
    Bool,
}

pub fn restrict(
    builder: &mut QueryBuilder<Postgres>,
    fixed: Option<&Predicate>,
    user: Option<&Predicate>,
    meta: &Metadata,
) -> Result<(), Error> {
    let predicates: Vec<_> = [fixed, user].into_iter().flatten().collect();
    if predicates.is_empty() {
        builder.push("TRUE");
    }
    for (i, predicate) in predicates.into_iter().enumerate() {
        if i > 0 {
            builder.push(" AND ");
        }
        compile(builder, predicate, meta)?;
    }
    Ok(())
}

pub fn compile(
    builder: &mut QueryBuilder<Postgres>,
    predicate: &Predicate,
    meta: &Metadata,
) -> Result<(), Error> {
    builder.push("(");
    match predicate {
        Predicate::And(args) | Predicate::Or(args) => {
            let joiner = if matches!(predicate, Predicate::And(_)) {
                " AND "
            } else {
                " OR "
            };
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    builder.push(joiner);
                }
                compile(builder, arg, meta)?;
            }
        }
        Predicate::Not(arg) => {
            builder.push("NOT ");
            compile(builder, arg, meta)?;
        }
        Predicate::Null { field, negated } => {
            let (column, _, _) = column(field, meta)?;
            builder
                .push(column)
                .push(if *negated { " IS NOT NULL" } else { " IS NULL" });
        }
        Predicate::In { field, values } => {
            let (column, kind, cast) = column(field, meta)?;
            builder.push(column).push(" IN (");
            for (i, value) in values.iter().enumerate() {
                if i > 0 {
                    builder.push(", ");
                }
                builder
                    .push_bind(literal(value, kind)?)
                    .push("::")
                    .push(cast);
            }
            builder.push(")");
        }
        Predicate::Compare { op, field, value } => {
            let (column, kind, cast) = column(field, meta)?;
            let operator = match op {
                Comparison::Eq => " = ",
                Comparison::Ne => " <> ",
                Comparison::Gt => " > ",
                Comparison::Gte => " >= ",
                Comparison::Lt => " < ",
                Comparison::Lte => " <= ",
                Comparison::Like if kind == Kind::Text => " LIKE ",
                Comparison::Ilike if kind == Kind::Text => " ILIKE ",
                _ => return Err(Error::InvalidFilter),
            };
            builder
                .push(column)
                .push(operator)
                .push_bind(literal(value, kind)?)
                .push("::")
                .push(cast);
        }
        Predicate::Spatial { .. } => return Err(Error::InvalidFilter),
    }
    builder.push(")");
    Ok(())
}

pub fn query_error(failure: sqlx::Error) -> Error {
    match failure {
        sqlx::Error::Database(ref database)
            if database.code().is_some_and(|code| code.starts_with("22")) =>
        {
            Error::InvalidFilter
        }
        other => error(other),
    }
}

fn column<'m>(field: &str, meta: &'m Metadata) -> Result<(String, Kind, &'m str), Error> {
    let attribute = meta
        .attributes
        .iter()
        .find(|a| a.name == field)
        .ok_or(Error::InvalidFilter)?;
    let kind = match attribute.data_type.as_str() {
        "text" | "varchar" | "bpchar" => Kind::Text,
        "date" | "timestamp" | "timestamptz" => Kind::Temporal,
        "int2" => Kind::Integer(i16::MIN.into(), i16::MAX.into()),
        "int4" => Kind::Integer(i32::MIN.into(), i32::MAX.into()),
        "int8" => Kind::Integer(i64::MIN, i64::MAX),
        "float4" => Kind::Float(f32::MAX.into()),
        "float8" => Kind::Float(f64::MAX),
        "numeric" => Kind::Numeric,
        "bool" => Kind::Bool,
        _ => return Err(Error::InvalidFilter),
    };
    Ok((quote_identifier(field), kind, &attribute.data_type))
}

fn literal(value: &Scalar, kind: Kind) -> Result<String, Error> {
    match (kind, value) {
        (Kind::Text, Scalar::Text(text)) => Ok(text.clone()),
        (Kind::Temporal, Scalar::Text(text)) if iso(text) => Ok(text.clone()),
        (Kind::Integer(min, max), Scalar::Number(n))
            if n.as_i64().is_some_and(|v| (min..=max).contains(&v)) =>
        {
            Ok(n.to_string())
        }
        (Kind::Float(max), Scalar::Number(n)) if n.as_f64().is_some_and(|v| v.abs() <= max) => {
            Ok(n.to_string())
        }
        (Kind::Numeric, Scalar::Number(n)) => Ok(n.to_string()),
        (Kind::Bool, Scalar::Bool(b)) => Ok(b.to_string()),
        _ => Err(Error::InvalidFilter),
    }
}

fn iso(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 10
        && bytes.len() <= 35
        && bytes[..10].iter().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
        && bytes[10..]
            .iter()
            .all(|b| b.is_ascii_digit() || b"-:.+TZ ".contains(b))
}
