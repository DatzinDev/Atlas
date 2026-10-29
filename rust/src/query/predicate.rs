use crate::{
    config::Limits,
    domain::{Error, identifier},
};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use serde_json::{Number, Value, value::RawValue};

#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
    Text(String),
    Number(Number),
    Bool(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    Like,
    Ilike,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Predicate {
    Compare {
        op: Comparison,
        field: String,
        value: Scalar,
    },
    In {
        field: String,
        values: Vec<Scalar>,
    },
    Null {
        field: String,
        negated: bool,
    },
}

pub fn parse(text: &str, limits: &Limits) -> Result<Predicate, Error> {
    if text.len() > limits.filter_bytes as usize {
        return Err(Error::InvalidFilter);
    }
    let raw: &RawValue = serde_json::from_str(text).map_err(invalid)?;
    Parser { limits, nodes: 0 }.predicate(raw, 1)
}

struct Parser<'l> {
    limits: &'l Limits,
    nodes: usize,
}

impl Parser<'_> {
    fn predicate(&mut self, raw: &RawValue, depth: u32) -> Result<Predicate, Error> {
        self.nodes += 1;
        if depth > self.limits.filter_depth || self.nodes > self.limits.filter_nodes as usize {
            return Err(Error::InvalidFilter);
        }
        let Pairs(pairs) = serde_json::from_str(raw.get()).map_err(invalid)?;
        let op = pairs
            .iter()
            .find(|(k, _)| k == "op")
            .ok_or(Error::InvalidFilter)
            .and_then(|(_, v)| text(v))?;
        Ok(match op.as_str() {
            "in" => {
                let [_, field, values] = take(&pairs, ["op", "field", "value"])?;
                let values: Vec<&RawValue> = serde_json::from_str(values.get()).map_err(invalid)?;
                if values.is_empty() || values.len() > self.limits.in_values as usize {
                    return Err(Error::InvalidFilter);
                }
                Predicate::In {
                    field: name(field)?,
                    values: values.into_iter().map(scalar).collect::<Result<_, _>>()?,
                }
            }
            "is_null" | "is_not_null" => {
                let [_, field] = take(&pairs, ["op", "field"])?;
                Predicate::Null {
                    field: name(field)?,
                    negated: op == "is_not_null",
                }
            }
            other => {
                let op = serde_json::from_value(other.into()).map_err(invalid)?;
                let [_, field, value] = take(&pairs, ["op", "field", "value"])?;
                Predicate::Compare {
                    op,
                    field: name(field)?,
                    value: scalar(value)?,
                }
            }
        })
    }
}

fn invalid(_: serde_json::Error) -> Error {
    Error::InvalidFilter
}

fn text(raw: &RawValue) -> Result<String, Error> {
    serde_json::from_str(raw.get()).map_err(invalid)
}

fn name(raw: &RawValue) -> Result<String, Error> {
    text(raw).and_then(|field| {
        identifier(&field)
            .then_some(field)
            .ok_or(Error::InvalidFilter)
    })
}

fn scalar(raw: &RawValue) -> Result<Scalar, Error> {
    match serde_json::from_str(raw.get()).map_err(invalid)? {
        Value::String(s) => Ok(Scalar::Text(s)),
        Value::Bool(b) => Ok(Scalar::Bool(b)),
        Value::Number(n) if n.as_f64().is_some_and(f64::is_finite) => Ok(Scalar::Number(n)),
        _ => Err(Error::InvalidFilter),
    }
}

fn take<'a, const N: usize>(
    pairs: &[(String, &'a RawValue)],
    keys: [&str; N],
) -> Result<[&'a RawValue; N], Error> {
    if pairs.len() != N {
        return Err(Error::InvalidFilter);
    }
    keys.iter()
        .filter_map(|k| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| *v))
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| Error::InvalidFilter)
}

struct Pairs<'a>(Vec<(String, &'a RawValue)>);

impl<'de> Deserialize<'de> for Pairs<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(PairsVisitor)
    }
}

struct PairsVisitor;

impl<'de> Visitor<'de> for PairsVisitor {
    type Value = Pairs<'de>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a predicate object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut pairs: Vec<(String, &'de RawValue)> = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if pairs.len() == 3 || pairs.iter().any(|(k, _)| *k == key) {
                return Err(de::Error::custom("duplicate or unexpected key"));
            }
            pairs.push((key, map.next_value()?));
        }
        Ok(Pairs(pairs))
    }
}
