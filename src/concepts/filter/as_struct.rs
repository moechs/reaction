use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::concepts::{Match, Pattern, Patterns};

use super::{FilterMethodAs, PatternSet, compute_regex_with_patterns};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilterMethodStruct {
    pub options: FilterMethodStructInner,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilterMethodStructInner {
    #[serde(default)]
    lang: Lang,
    fields: HashMap<String, Field>,
    #[serde(skip)]
    computed_fields: HashMap<String, ComputedField>,
}

#[derive(Default, Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub enum Lang {
    #[serde(rename = "json", alias = "JSON")]
    #[default]
    JSON,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Field {
    Short(String),
    Long(LongField),
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LongField {
    regex: Option<String>,
    exists: Option<bool>,
    equals: Option<ScalarValue>,
    #[serde(flatten)]
    inner_fields: HashMap<String, Field>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ScalarValue {
    String(String),
    Bool(bool),
    Integer(i64),
}

impl Field {
    fn compute(
        self,
        config_patterns: &Patterns,
    ) -> Result<(ComputedField, BTreeSet<Arc<Pattern>>), String> {
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
            ) => {
                let (str_regex, regex, patterns) =
                    compute_regex_with_patterns(regex, config_patterns)?;

                Ok((ComputedField::Regex((str_regex, regex)), patterns))
            }
            (
                0,
                LongField {
                    regex: None,
                    exists: Some(bool),
                    equals: None,
                    inner_fields: _,
                },
            ) => Ok((ComputedField::Exists(bool), Default::default())),
            (
                0,
                LongField {
                    regex: None,
                    exists: None,
                    equals: Some(scalar),
                    inner_fields: _,
                },
            ) => Ok((ComputedField::Equals(scalar), Default::default())),
            (
                0,
                LongField {
                    regex: None,
                    exists: None,
                    equals: None,
                    inner_fields: _,
                },
            ) => Err("no test set. you must set at least one test".into()),
            (
                _,
                LongField {
                    regex: None,
                    exists: None,
                    equals: None,
                    inner_fields,
                },
            ) => {
                // Recurse into inner_field.compute()
                let (inner, patterns) = Self::compute_map(inner_fields, config_patterns)?;
                Ok((ComputedField::Inner(inner), patterns))
            }
            (_longfield, _len) => Err(
                "multiple tests set. Must be only one of `regex`, `exists`, `equals`, or inner fields if it's an object."
                    .into(),
            ),
        }
    }

    fn compute_map(
        fields: HashMap<String, Field>,
        config_patterns: &Patterns,
    ) -> Result<(HashMap<String, ComputedField>, BTreeSet<Arc<Pattern>>), String> {
        let mut inner = HashMap::default();
        let mut patterns = BTreeSet::default();

        for (key, field) in fields.into_iter() {
            let (computed_field, p) = field
                .compute(config_patterns)
                .map_err(|err| format!("{key}: {err}"))?;
            inner.insert(key, computed_field);
            patterns.extend(p);
        }
        Ok((inner, patterns))
    }
}

#[derive(Debug, Clone)]
pub enum ComputedField {
    Regex((String, Regex)),
    Exists(bool),
    Equals(ScalarValue),
    Inner(HashMap<String, ComputedField>),
}

impl PartialEq for ComputedField {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Regex(l0), Self::Regex(r0)) => l0.0 == r0.0,
            (Self::Exists(l0), Self::Exists(r0)) => l0 == r0,
            (Self::Equals(l0), Self::Equals(r0)) => l0 == r0,
            (Self::Inner(l0), Self::Inner(r0)) => l0 == r0,
            _ => false,
        }
    }
}
impl Eq for ComputedField {}

impl FilterMethodAs for FilterMethodStruct {
    fn setup(&mut self, config_patterns: &Patterns) -> Result<Arc<PatternSet>, String> {
        let (computed_fields, patterns) =
            Field::compute_map(std::mem::take(&mut self.options.fields), config_patterns)?;
        self.options.computed_fields = computed_fields;

        Ok(Arc::new(patterns))
    }

    fn get_match(&self, line: &str) -> Option<Match> {
        todo!()
    }

    fn hint_regexes(&self) -> Vec<String> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use regex::Regex;

    use crate::concepts::{
        Patterns,
        filter::{
            FilterMethodAs,
            as_struct::{ComputedField, FilterMethodStruct, FilterMethodStructInner, ScalarValue},
        },
    };

    #[test]
    fn no_lang() {
        let mut fms: FilterMethodStruct = serde_json::from_str(
            r#"
            {
                "options": {
                    "fields": {
                        "a": "it's a regex"
                    }
                }
            }
            "#,
        )
        .unwrap();
        fms.setup(&Patterns::default()).unwrap();
    }

    #[test]
    fn lang_json() {
        let mut fms: FilterMethodStruct = serde_json::from_str(
            r#"
            {
                "options": {
                    "lang": "json",
                    "fields": {
                        "a": "it's a regex"
                    }
                }
            }
            "#,
        )
        .unwrap();
        fms.setup(&Patterns::default()).unwrap();
    }

    #[test]
    fn lang_invalid() {
        serde_json::from_str::<FilterMethodStruct>(
            r#"
            {
                "options": {
                    "lang": "jason",
                    "fields": {
                        "a": "it's a regex"
                    }
                }
            }
            "#,
        )
        .unwrap_err();
    }

    #[test]
    fn short_field() {
        let mut fms: FilterMethodStruct = serde_json::from_str(
            r#"
            {
                "options": {
                    "fields": {
                        "a": "it's a regex"
                    }
                }
            }
            "#,
        )
        .unwrap();
        fms.setup(&Patterns::default()).unwrap();
        assert_eq!(
            fms.options,
            FilterMethodStructInner {
                computed_fields: HashMap::from([(
                    "a".into(),
                    ComputedField::Regex((
                        "it's a regex".into(),
                        Regex::new("it's a regex").unwrap()
                    ))
                )]),
                ..Default::default()
            }
        );
    }

    #[test]
    fn long_field() {
        let mut fms: FilterMethodStruct = serde_json::from_str(
            r#"
            {
                "options": {
                    "fields": {
                        "a": {
                            "regex": "it's a regex"
                        }
                    }
                }
            }
            "#,
        )
        .unwrap();
        fms.setup(&Patterns::default()).unwrap();
        assert_eq!(
            fms.options,
            FilterMethodStructInner {
                computed_fields: HashMap::from([(
                    "a".into(),
                    ComputedField::Regex((
                        "it's a regex".into(),
                        Regex::new("it's a regex").unwrap()
                    ))
                )]),
                ..Default::default()
            }
        );
    }

    #[test]
    fn invalid_multiple_values_on_field() {
        let mut fms: FilterMethodStruct = serde_json::from_str(
            r#"
            {
                "options": {
                    "fields": {
                        "a": {
                            "regex": "it's a regex",
                            "exists": true
                        }
                    }
                }
            }
            "#,
        )
        .unwrap();
        fms.setup(&Patterns::default()).unwrap_err();
    }

    #[test]
    fn multiple_fields() {
        let mut fms: FilterMethodStruct = serde_json::from_str(
            r#"
            {
                "options": {
                    "fields": {
                        "a": {
                            "regex": "it's a regex"
                        },
                        "b": "it's a regex",
                        "c": {
                            "exists": true
                        },
                        "d": {
                            "exists": false
                        },
                        "e": {
                            "equals": 1
                        },
                        "f": {
                            "equals": "yay"
                        },
                        "g": {
                            "equals": true
                        },
                        "h": {
                            "equals": false
                        },
                        "i": {
                            "b": "it's a regex",
                            "c": {
                                "exists": true
                            },
                            "i": {
                                "b": "it's a regex",
                                "c": {
                                    "exists": true
                                }
                            }
                        }
                    }
                }
            }
            "#,
        )
        .unwrap();
        fms.setup(&Patterns::default()).unwrap();
        assert_eq!(
            fms.options,
            FilterMethodStructInner {
                computed_fields: HashMap::from([
                    (
                        "a".into(),
                        ComputedField::Regex((
                            "it's a regex".into(),
                            Regex::new("it's a regex").unwrap()
                        ))
                    ),
                    (
                        "b".into(),
                        ComputedField::Regex((
                            "it's a regex".into(),
                            Regex::new("it's a regex").unwrap()
                        ))
                    ),
                    ("c".into(), ComputedField::Exists(true)),
                    ("d".into(), ComputedField::Exists(false)),
                    ("e".into(), ComputedField::Equals(ScalarValue::Integer(1))),
                    (
                        "f".into(),
                        ComputedField::Equals(ScalarValue::String("yay".into()))
                    ),
                    ("g".into(), ComputedField::Equals(ScalarValue::Bool(true))),
                    ("h".into(), ComputedField::Equals(ScalarValue::Bool(false))),
                    (
                        "i".into(),
                        ComputedField::Inner(HashMap::from([
                            (
                                "b".into(),
                                ComputedField::Regex((
                                    "it's a regex".into(),
                                    Regex::new("it's a regex").unwrap()
                                ))
                            ),
                            ("c".into(), ComputedField::Exists(true)),
                            (
                                "i".into(),
                                ComputedField::Inner(HashMap::from([
                                    (
                                        "b".into(),
                                        ComputedField::Regex((
                                            "it's a regex".into(),
                                            Regex::new("it's a regex").unwrap()
                                        ))
                                    ),
                                    ("c".into(), ComputedField::Exists(true)),
                                ]))
                            ),
                        ]))
                    ),
                ]),
                ..Default::default()
            }
        );
    }
}
