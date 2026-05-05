use std::collections::{BTreeMap, HashMap};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::concepts::{Match, Patterns};

use super::FilterMethodAs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterMethodStruct {
    pub options: FilterMethodStructInner,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterMethodStructInner {
    lang: Lang,
    fields: BTreeMap<String, Field>,
    #[serde(skip)]
    computed_fields: BTreeMap<String, ComputedField>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub enum Lang {
    #[serde(rename = "json", alias = "JSON")]
    JSON,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Field {
    Short(String),
    Long(LongField),
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct LongField {
    regex: Option<String>,
    exists: Option<bool>,
    equals: Option<ScalarValue>,
    #[serde(flatten)]
    inner_fields: HashMap<String, Field>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScalarValue {
    String(String),
    Bool(bool),
    Integer(i64),
}

impl Field {
    fn compute(self) -> Result<ComputedField, String> {
        let field = match self {
            Field::Short(regex) => LongField {
                regex: Some(regex),
                ..Default::default()
            },
            Field::Long(l) => l,
        };

        match (field.inner_fields.len(), field) {
            (
                0,
                LongField {
                    regex: Some(regex),
                    exists: None,
                    equals: None,
                    inner_fields: _,
                },
            ) => Ok(ComputedField::Regex(
                Regex::new(&regex).map_err(|err| format!("{err}"))?,
            )),
            (
                0,
                LongField {
                    regex: None,
                    exists: Some(bool),
                    equals: None,
                    inner_fields: _,
                },
            ) => Ok(ComputedField::Exists(bool)),
            (
                0,
                LongField {
                    regex: None,
                    exists: None,
                    equals: Some(scalar),
                    inner_fields: _,
                },
            ) => Ok(ComputedField::Equals(scalar)),
            (
                0,
                LongField {
                    regex: None,
                    exists: None,
                    equals: None,
                    inner_fields: _,
                },
            ) => Err("no value".into()),
            (
                _,
                LongField {
                    regex: None,
                    exists: None,
                    equals: None,
                    inner_fields: inner,
                },
            ) => Ok(ComputedField::Inner(
                inner
                    .into_iter()
                    .map(|(k, v)| v.compute().map(|v| (k, v)))
                    .collect::<Result<HashMap<String, ComputedField>, String>>()?,
            )),
            (_longfield, _len) => Err("multiple values".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ComputedField {
    Regex(Regex),
    Exists(bool),
    Equals(ScalarValue),
    Inner(HashMap<String, ComputedField>),
}

impl FilterMethodAs for FilterMethodStruct {
    fn setup(&mut self, config_patterns: &Patterns) -> Result<super::PatternSet, String> {
        todo!()
    }

    fn get_match(&self, line: &str) -> Option<Match> {
        todo!()
    }

    fn hint_regexes(&self) -> Vec<String> {
        todo!()
    }
}
