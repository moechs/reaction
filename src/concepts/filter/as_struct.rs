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
    equals: Option<Value>,
    #[serde(flatten)]
    inner_fields: HashMap<String, Field>,
}

fn prepare_regex(regex: &mut String) {
    // trim leading '^', will be replaced by multi-line compatible '\A'
    if regex.starts_with("^") {
        regex.remove(0);
    }
    // trim ending '$', will be replaced by multi-line compatible '\z'
    if regex.ends_with("$") {
        regex.remove(regex.len() - 1);
    }
    // switch to multiline mode and insert leading '\A'
    regex.insert_str(0, "(?m)\\A");
    // insert ending '\z'
    regex.insert_str(regex.len(), "\\z");
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
                    regex: Some(mut regex),
                    exists: None,
                    equals: None,
                    inner_fields: _,
                },
            ) => {
                prepare_regex(&mut regex);

                let (_, regex, patterns) =
                    compute_regex_with_patterns(regex, config_patterns)?;

                Ok((
                    ComputedField::Regex((
                        regex,
                        // The indexes must be computed after that
                        patterns.iter().map(|pattern| (pattern.clone(), 0)).collect()
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
    Regex((Regex, Vec<(Arc<Pattern>, usize)>)),
    Exists(bool),
    Equals(Value),
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

fn regex_matches(
    regex: &Regex,
    patterns: &Vec<(Arc<Pattern>, usize)>,
    actual_value: &str,
    match_: &mut Match,
) -> bool {
    if let Some(matches) = regex.captures(actual_value) {
        for (pattern, pattern_index) in patterns {
            // if the pattern is in an optional part of the regex,
            // there may be no captured group for it.
            if let Some(match_elt) = matches.name(&pattern.name) {
                let mut match_elt = match_elt.as_str().to_string();
                pattern.normalize(&mut match_elt);
                match_[*pattern_index] = match_elt;
            } else {
                return false;
            }
        }
        true
    } else {
        false
    }
}

impl ComputedField {
    fn post_compute(&mut self, used_patterns: &PatternSet) {
        if let Self::Regex((_, patterns)) = self {
            for i in 0..patterns.len() {
                patterns[i].1 = used_patterns
                    .iter()
                    .enumerate()
                    .find(|(_, pattern)| pattern.as_ref() == patterns[i].0.as_ref())
                    .unwrap()
                    .0;
            }
        }
    }

    fn post_compute_map(this: &mut HashMap<String, Self>, used_patterns: &PatternSet) {
        this.values_mut()
            .for_each(|field| field.post_compute(used_patterns));
    }

    fn matches(&self, actual_value: Option<&Value>, match_: &mut Match) -> bool {
        match self {
            ComputedField::Regex((regex, patterns)) => match actual_value {
                Some(Value::String(actual_value)) => {
                    regex_matches(regex, patterns, actual_value, match_)
                }
                Some(Value::Number(actual_value)) => {
                    regex_matches(regex, patterns, actual_value.as_str(), match_)
                }
                _ => false,
            },
            ComputedField::Exists(exists) => *exists == actual_value.is_some(),
            ComputedField::Equals(scalar_value) => {
                actual_value.is_some_and(|actual_value| scalar_value == actual_value)
            }
            ComputedField::Inner(contract_map) => match actual_value {
                Some(Value::Object(_)) => {
                    Self::matches_all(contract_map, actual_value.unwrap(), match_)
                }
                _ => false,
            },
        }
    }

    fn matches_all(
        this: &HashMap<String, Self>,
        actual_values: &Value,
        match_: &mut Match,
    ) -> bool {
        for (contract_key, contract_value) in this {
            let actual_value = actual_values.get(contract_key);
            let ok = contract_value.matches(actual_value, match_);
            if !ok {
                return false;
            }
        }
        return true;
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

        let mut match_: Match = (0..self.patterns.len())
            .map(|_| String::default())
            .collect();

        Ok(
            if ComputedField::matches_all(&self.computed_fields, &value, &mut match_) {
                Some(match_)
            } else {
                None
            },
        )
    }

    fn hint_regexes(&self) -> Option<Vec<String>> {
        None
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
            as_struct::{ComputedField, FilterMethodStruct, prepare_regex},
        },
    };

    #[test]
    fn test_prepare_regex() {
        for regex in ["my_regex", "^my_regex", "my_regex$", "^my_regex$"] {
            let mut s = regex.to_string();
            prepare_regex(&mut s);
            assert_eq!(&s, "(?m)\\Amy_regex\\z");
        }
    }

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
                    ComputedField::Regex((Regex::new("(?m)\\Ait's a regex\\z").unwrap(), vec![]))
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
                    ComputedField::Regex((Regex::new("(?m)\\Ait's a regex\\z").unwrap(), vec![]))
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
                        ComputedField::Regex((
                            Regex::new("(?m)\\Ait's a regex\\z").unwrap(),
                            vec![]
                        ))
                    ),
                    (
                        "b".into(),
                        ComputedField::Regex((
                            Regex::new("(?m)\\Ait's a regex\\z").unwrap(),
                            vec![]
                        ))
                    ),
                    ("c".into(), ComputedField::Exists(true)),
                    ("d".into(), ComputedField::Exists(false)),
                    ("e".into(), ComputedField::Equals(1.into())),
                    ("f".into(), ComputedField::Equals("yay".into())),
                    ("g".into(), ComputedField::Equals(true.into())),
                    ("h".into(), ComputedField::Equals(false.into())),
                    (
                        "i".into(),
                        ComputedField::Inner(HashMap::from([
                            (
                                "b".into(),
                                ComputedField::Regex((
                                    Regex::new("(?m)\\Ait's a regex\\z").unwrap(),
                                    vec![]
                                ))
                            ),
                            ("c".into(), ComputedField::Exists(true)),
                            (
                                "i".into(),
                                ComputedField::Inner(HashMap::from([
                                    (
                                        "b".into(),
                                        ComputedField::Regex((
                                            Regex::new("(?m)\\Ait's a regex\\z").unwrap(),
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
                        Regex::new("(?m)\\A(?P<name>[a-z]+)\\z").unwrap(),
                        vec![(patterns["name"].clone(), 0)]
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
                        Regex::new("(?m)\\A(?P<name>[a-z]+) - (?P<age>\\d+)\\z").unwrap(),
                        vec![(patterns["age"].clone(), 0), (patterns["name"].clone(), 1)]
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
                            Regex::new("(?m)\\Amy name is (?P<name>[a-z]+)\\z").unwrap(),
                            vec![(patterns["name"].clone(), 1)]
                        ))
                    ),
                    (
                        "b".into(),
                        ComputedField::Regex((
                            Regex::new("(?m)\\Amy age is (?P<age>\\d+)\\z").unwrap(),
                            vec![(patterns["age"].clone(), 0)]
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

    #[test]
    fn get_match() {
        let tests = [
            (
                "test regex",
                r#"
                {
                    "options": {
                        "fields": {
                            "a": "<name>"
                        }
                    }
                }
                "#,
                vec![
                    (r#" {"a": "hello"} "#, Ok(Some(vec!["hello".into()]))),
                    (
                        r#" {"a": "hello", "b": 12} "#,
                        Ok(Some(vec!["hello".into()])),
                    ),
                    (r#" {} "#, Ok(None)),
                    (r#" {"a": "Hello"} "#, Ok(None)),
                    (r#" {"b": "hello"} "#, Ok(None)),
                ],
            ),
            (
                "test exists true",
                r#"
                {
                    "options": {
                        "fields": {
                            "a": {
                                "exists": true
                            }
                        }
                    }
                }
                "#,
                vec![
                    (r#" {"a": "hello"} "#, Ok(Some(vec![]))),
                    (r#" {"a": ["hey"], "b": 12} "#, Ok(Some(vec![]))),
                    (r#" {} "#, Ok(None)),
                    (r#" {"b": "hello"} "#, Ok(None)),
                ],
            ),
            (
                "test exists false",
                r#"
                {
                    "options": {
                        "fields": {
                            "a": {
                                "exists": false
                            }
                        }
                    }
                }
                "#,
                vec![
                    (r#" {"a": "hello"} "#, Ok(None)),
                    (r#" {"a": ["hey"], "b": 12} "#, Ok(None)),
                    (r#" {} "#, Ok(Some(vec![]))),
                    (r#" {"b": "hello"} "#, Ok(Some(vec![]))),
                ],
            ),
            (
                "simple equals bool",
                r#"
                {
                    "options": {
                        "fields": {
                            "a": {
                                "equals": true
                            }
                        }
                    }
                }
                "#,
                vec![
                    (r#" {"a": true} "#, Ok(Some(vec![]))),
                    (r#" {"a": false} "#, Ok(None)),
                    (r#" {} "#, Ok(None)),
                    (r#" {"b": "hello"} "#, Ok(None)),
                ],
            ),
            (
                "complex equals",
                r#"
                {
                    "options": {
                        "fields": {
                            "a": {
                                "equals": [1, null, "prout", {"yay": true}]
                            }
                        }
                    }
                }
                "#,
                vec![
                    (
                        r#" {"a": [1, null, "prout", {"yay": true}]} "#,
                        Ok(Some(vec![])),
                    ),
                    (r#" {"a": [1, false, "prout", {"yay": true}]} "#, Ok(None)),
                    (r#" {"a": [1, null, "prout", {"yay": "true"}]} "#, Ok(None)),
                    (r#" {"a": "yay"} "#, Ok(None)),
                    (r#" {} "#, Ok(None)),
                    (r#" {"b": "hello"} "#, Ok(None)),
                ],
            ),
            (
                "test regex and exists and equals and inner",
                r#"
                {
                    "options": {
                        "fields": {
                            "a": "^<name>$",
                            "b": {
                                "c": "[0-9a-f]+",
                                "d": {
                                    "e": { "exists": true }
                                },
                                "f": { "equals": [1,2] },
                                "g": { "exists": false }
                            }
                        }
                    }
                }
                "#,
                vec![
                    (
                        r#" {"a":"prdb","b":{"c":"4c4b","d":{"e": 0,"z": 1},"f":[1,2],"h":"ok"}} "#,
                        Ok(Some(vec!["prdb".into()])),
                    ),
                    (
                        // regex on a number stringifies it
                        r#" {"a":"prdb","b":{"c":1234567,"d":{"e": 0,"z": 1},"f":[1,2],"h":"ok"}} "#,
                        Ok(Some(vec!["prdb".into()])),
                    ),
                    (
                        r#" {"a":"pr2b","b":{"c":"4czb","d":{"e": 0,"z": 1},"f":[1,2],"h":"ok"}} "#,
                        Ok(None),
                    ),
                    (
                        r#" {"a":"prdb","b":{"c":"4c4b","d":{"E": 0,"z": 1},"f":[1,2],"h":"ok"}} "#,
                        Ok(None),
                    ),
                    (
                        r#" {"a":"prdb","b":{"c":"4c4b","d":{"e": 0,"z": 1},"f":[1,2,3],"h":"ok"}} "#,
                        Ok(None),
                    ),
                    (
                        r#" {"a":"prdb","b":{"c":"4c4b","d":{"e": 0,"z": 1},"f":[1,2],"g":"ok"}} "#,
                        Ok(None),
                    ),
                ],
            ),
            (
                "test traefik log",
                r#"
                {
                    "options": {
                        "fields": {
                            "ClientHost": "<ip>",
                            "DownstreamStatus": {
                                "equals": 401
                            },
                            "RequestPath": {
                                "equals": "/login"
                            }
                        }
                    }
                }
                "#,
                vec![
                    (
                        r#" { "ClientAddr": "1.2.3.4:2048", "ClientHost": "1.2.3.4", "ClientPort": "2048", "DownstreamContentSize": 252, "DownstreamStatus": 200, "Duration": 1000, "OriginContentSize": 252, "OriginDuration": 900, "OriginStatus": 206, "Overhead": 10000, "RequestAddr": "domain.name", "RequestContentSize": 0, "RequestCount": 123, "RequestHost": "domain.name", "RequestMethod": "GET", "RequestPath": "/login", "RequestPort": "-", "RequestProtocol": "HTTP/2.0", "RequestScheme": "https", "RetryAttempts": 0, "RouterName": "my-service@docker", "ServiceAddr": "172.1.0.1:80", "ServiceName": "my-service@docker", "downstream_Header1": "...", "downstream_Header2": "...", "entryPointName": "websecure", "level": "info", "msg": "", "origin_Header1": "...", "origin_Header2": "...", "request_Header1": "...", "request_Header2": "...", "time": "YYYY-MM-DDTHH:MM:SS+UT:C0" } "#,
                        Ok(None),
                    ),
                    (
                        r#" { "ClientAddr": "1.2.3.4:2048", "ClientHost": "1.2.3.4", "ClientPort": "2048", "DownstreamContentSize": 252, "DownstreamStatus": 401, "Duration": 1000, "OriginContentSize": 252, "OriginDuration": 900, "OriginStatus": 206, "Overhead": 10000, "RequestAddr": "domain.name", "RequestContentSize": 0, "RequestCount": 123, "RequestHost": "domain.name", "RequestMethod": "GET", "RequestPath": "/login", "RequestPort": "-", "RequestProtocol": "HTTP/2.0", "RequestScheme": "https", "RetryAttempts": 0, "RouterName": "my-service@docker", "ServiceAddr": "172.1.0.1:80", "ServiceName": "my-service@docker", "downstream_Header1": "...", "downstream_Header2": "...", "entryPointName": "websecure", "level": "info", "msg": "", "origin_Header1": "...", "origin_Header2": "...", "request_Header1": "...", "request_Header2": "...", "time": "YYYY-MM-DDTHH:MM:SS+UT:C0" } "#,
                        Ok(Some(vec!["1.2.3.4".into()])),
                    ),
                ],
            ),
        ];
        for (name, test, tries) in tests {
            let mut fms: FilterMethodStruct = serde_json::from_str(test).unwrap();

            let mut patterns = Pattern::new_map("name", "[a-z]+").unwrap();
            patterns.extend(Pattern::new_map("age", "\\d+").unwrap());
            patterns.insert("ip".into(), {
                let mut pattern: Pattern = serde_json::from_str(r#"{"type": "ip"}"#).unwrap();
                pattern.setup("ip").unwrap();
                pattern.into()
            });
            fms.setup(&patterns).unwrap();

            for (arg, result) in tries {
                assert_eq!(
                    fms.get_match(arg),
                    result,
                    "{name} (arg: {arg}, result: {result:?})"
                );
            }
        }
    }
}
