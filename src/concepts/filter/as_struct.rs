use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::concepts::{Match, Pattern, Patterns};

use super::{FilterMethodAs, PatternSet, compute_regex_with_patterns};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(Default, PartialEq, Eq))]
pub struct FilterMethodStruct {
    pub options: FilterMethodStructInner,
    #[serde(skip)]
    computed_fields: HashMap<String, ComputedField>,
    #[serde(skip)]
    patterns: Arc<PatternSet>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct FilterMethodStructInner {
    #[serde(default)]
    lang: Lang,
    fields: HashMap<String, Field>,
}

#[derive(Default, Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub enum Lang {
    #[serde(rename = "json", alias = "JSON")]
    #[default]
    JSON,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq, Eq))]
#[serde(untagged)]
pub enum Field {
    Short(String),
    Long(LongField),
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq, Eq))]
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
                let (_, regex, patterns) =
                    compute_regex_with_patterns(regex, config_patterns)?;

                Ok((
                    ComputedField::Regex((
                        regex,
                        // The indexes must be computed after that
                        patterns.iter().map(|pattern| (pattern.name.clone(), 0)).collect()
                    )),
                    patterns
                ))
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

            if let Some(pattern) = patterns.intersection(&p).next() {
                return Err(format!(
                    "pattern {} present in multiple fields. Each pattern must be present at most one time.",
                    pattern.name_with_braces()
                ));
            }
            patterns.extend(p);
        }
        Ok((inner, patterns))
    }
}

#[derive(Debug, Clone)]
pub enum ComputedField {
    Regex((Regex, Vec<(String, usize)>)),
    Exists(bool),
    Equals(ScalarValue),
    Inner(HashMap<String, ComputedField>),
}

#[cfg(test)]
impl PartialEq for ComputedField {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Regex(l0), Self::Regex(r0)) => l0.0.as_str() == r0.0.as_str() && l0.1 == r0.1,
            (Self::Exists(l0), Self::Exists(r0)) => l0 == r0,
            (Self::Equals(l0), Self::Equals(r0)) => l0 == r0,
            (Self::Inner(l0), Self::Inner(r0)) => l0 == r0,
            _ => false,
        }
    }
}
#[cfg(test)]
impl Eq for ComputedField {}

impl ComputedField {
    fn post_compute(&mut self, used_patterns: &PatternSet) {
        if let Self::Regex((_, patterns)) = self {
            for i in 0..patterns.len() {
                patterns[i].1 = used_patterns
                    .iter()
                    .enumerate()
                    .find(|pattern| &pattern.1.name == &patterns[i].0)
                    .unwrap()
                    .0;
            }
        }
    }

    fn post_compute_map(this: &mut HashMap<String, Self>, used_patterns: &PatternSet) {
        this.values_mut()
            .for_each(|field| field.post_compute(used_patterns));
    }

    fn matches(&self, value: Value) -> Option<PatternSet> {
        todo!()
    }

    fn matches_all(this: &HashMap<String, Self>, values: Value) -> bool {
        todo!()
    }
}

impl FilterMethodAs for FilterMethodStruct {
    fn setup(&mut self, config_patterns: &Patterns) -> Result<Arc<PatternSet>, String> {
        let (mut computed_fields, patterns) =
            Field::compute_map(std::mem::take(&mut self.options.fields), config_patterns)?;

        ComputedField::post_compute_map(&mut computed_fields, &patterns);

        self.computed_fields = computed_fields;
        self.patterns = Arc::new(patterns);
        Ok(self.patterns.clone())
    }

    fn get_match(&self, line: &str) -> Result<Option<Match>, String> {
        let value = match self.options.lang {
            Lang::JSON => serde_json::from_str::<serde_json::Value>(line)
                .map_err(|err| format!("line is not valid JSON: {err}"))?,
        };

        ComputedField::matches_all(&self.computed_fields, value);
        todo!()
    }

    fn hint_regexes(&self) -> Vec<String> {
        // TODO generate smarter hints
        vec![".".into()]
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use regex::Regex;

    use crate::concepts::{
        Pattern, Patterns,
        filter::{
            FilterMethodAs,
            as_struct::{ComputedField, FilterMethodStruct, ScalarValue},
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
            fms,
            FilterMethodStruct {
                computed_fields: HashMap::from([(
                    "a".into(),
                    ComputedField::Regex((Regex::new("it's a regex").unwrap(), vec![]))
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
            fms,
            FilterMethodStruct {
                computed_fields: HashMap::from([(
                    "a".into(),
                    ComputedField::Regex((Regex::new("it's a regex").unwrap(), vec![]))
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
            fms,
            FilterMethodStruct {
                computed_fields: HashMap::from([
                    (
                        "a".into(),
                        ComputedField::Regex((Regex::new("it's a regex").unwrap(), vec![]))
                    ),
                    (
                        "b".into(),
                        ComputedField::Regex((Regex::new("it's a regex").unwrap(), vec![]))
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
                                ComputedField::Regex((Regex::new("it's a regex").unwrap(), vec![]))
                            ),
                            ("c".into(), ComputedField::Exists(true)),
                            (
                                "i".into(),
                                ComputedField::Inner(HashMap::from([
                                    (
                                        "b".into(),
                                        ComputedField::Regex((
                                            Regex::new("it's a regex").unwrap(),
                                            vec![]
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

    #[test]
    fn field_with_pattern() {
        let mut fms: FilterMethodStruct = serde_json::from_str(
            r#"
            {
                "options": {
                    "fields": {
                        "a": "<name>"
                    }
                }
            }
            "#,
        )
        .unwrap();

        let patterns = Pattern::new_map("name", "[a-z]+").unwrap();
        fms.setup(&patterns).unwrap();

        assert_eq!(
            fms,
            FilterMethodStruct {
                computed_fields: HashMap::from([(
                    "a".into(),
                    ComputedField::Regex((
                        Regex::new("(?P<name>[a-z]+)").unwrap(),
                        vec![("name".into(), 0)]
                    ))
                )]),
                patterns: Arc::new(patterns.into_values().collect()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn field_with_two_patterns() {
        let mut fms: FilterMethodStruct = serde_json::from_str(
            r#"
            {
                "options": {
                    "fields": {
                        "a": "<name> - <age>"
                    }
                }
            }
            "#,
        )
        .unwrap();

        let mut patterns = Pattern::new_map("name", "[a-z]+").unwrap();
        patterns.extend(Pattern::new_map("age", "\\d+").unwrap());

        fms.setup(&patterns).unwrap();

        assert_eq!(
            fms,
            FilterMethodStruct {
                computed_fields: HashMap::from([(
                    "a".into(),
                    ComputedField::Regex((
                        Regex::new("(?P<name>[a-z]+) - (?P<age>\\d+)").unwrap(),
                        vec![("age".into(), 0), ("name".into(), 1)]
                    ))
                )]),
                patterns: Arc::new(patterns.into_values().collect()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn field_with_duplicated_patterns() {
        let mut fms: FilterMethodStruct = serde_json::from_str(
            r#"
            {
                "options": {
                    "fields": {
                        "a": "<age> - <age>"
                    }
                }
            }
            "#,
        )
        .unwrap();

        let mut patterns = Pattern::new_map("name", "[a-z]+").unwrap();
        patterns.extend(Pattern::new_map("age", "\\d+").unwrap());

        fms.setup(&patterns).unwrap_err();
    }

    #[test]
    fn fields_with_two_patterns() {
        let mut fms: FilterMethodStruct = serde_json::from_str(
            r#"
            {
                "options": {
                    "fields": {
                        "a": "my name is <name>",
                        "b": "my age is <age>"
                    }
                }
            }
            "#,
        )
        .unwrap();

        let mut patterns = Pattern::new_map("name", "[a-z]+").unwrap();
        patterns.extend(Pattern::new_map("age", "\\d+").unwrap());

        fms.setup(&patterns).unwrap();

        assert_eq!(
            fms,
            FilterMethodStruct {
                computed_fields: HashMap::from([
                    (
                        "a".into(),
                        ComputedField::Regex((
                            Regex::new("my name is (?P<name>[a-z]+)").unwrap(),
                            vec![("name".into(), 1)]
                        ))
                    ),
                    (
                        "b".into(),
                        ComputedField::Regex((
                            Regex::new("my age is (?P<age>\\d+)").unwrap(),
                            vec![("age".into(), 0)]
                        ))
                    )
                ]),
                patterns: Arc::new(patterns.into_values().collect()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn fields_with_duplicated_patterns() {
        let mut fms: FilterMethodStruct = serde_json::from_str(
            r#"
            {
                "options": {
                    "fields": {
                        "a": "hey, I'm <age>.",
                        "b": "hey, I'm <age>."
                    }
                }
            }
            "#,
        )
        .unwrap();

        let mut patterns = Pattern::new_map("name", "[a-z]+").unwrap();
        patterns.extend(Pattern::new_map("age", "\\d+").unwrap());

        fms.setup(&patterns).unwrap_err();
    }
}
