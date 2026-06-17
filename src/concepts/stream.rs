use std::{cmp::Ordering, collections::BTreeMap, fmt::Display, hash::Hash, time::Duration};

use reaction_plugin::{
    StreamConfig,
    time::{deser_opt_duration, ser_opt_duration},
};
use regex::RegexSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Filter, Patterns, merge_attrs};

#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Eof {
    behavior: Behavior,
    #[serde(
        default,
        serialize_with = "ser_opt_duration",
        deserialize_with = "deser_opt_duration"
    )]
    after: Option<Duration>,
    #[serde(default)]
    retry: u32,
    #[serde(default)]
    finally: Option<FinallyBehavior>,
}

impl Eof {
    pub fn is_default(&self) -> bool {
        *self == Eof::default()
    }

    pub fn setup(&mut self) -> Result<(), String> {
        match self.behavior {
            Behavior::Restart => {
                if let Some(finally) = &self.finally
                    && self.retry == 0
                {
                    return Err(format!(
                        "eof.finally is set to {finally} but there is no retry limit (eof.retry == 0). finally will never happen!"
                    ));
                }
                if self.finally.is_none() && self.retry != 0 {
                    return Err(format!(
                        "eof.finally is not set but there is a retry limit (eof.retry != 0). you must tell what happen after the retries!"
                    ));
                }

                if self.after.is_none() {
                    self.after = Some(Duration::from_secs(5));
                }
            }
            _ => {
                if self.retry != 0 {
                    return Err("eof.retry is set but eof.behavior is not set to 'restart'".into());
                }
                if self.after.is_some() {
                    return Err("eof.after is set but eof.behavior is not set to 'restart'".into());
                }
                if self.finally.is_some() {
                    return Err(
                        "eof.finally is set but eof.behavior is not set to 'restart'".into(),
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Behavior {
    #[default]
    #[serde(rename = "ignore")]
    Ignore,
    #[serde(rename = "exit")]
    Exit,
    #[serde(rename = "restart")]
    Restart,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum FinallyBehavior {
    #[serde(rename = "ignore")]
    Ignore,
    #[serde(rename = "exit")]
    Exit,
}
impl Display for FinallyBehavior {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FinallyBehavior::Ignore => write!(f, "ignore"),
            FinallyBehavior::Exit => write!(f, "exit"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(Default))]
#[serde(deny_unknown_fields)]
pub struct Stream {
    #[serde(default)]
    pub cmd: Vec<String>,
    #[serde(default, skip_serializing_if = "Eof::is_default")]
    pub eof: Eof,

    #[serde(default)]
    pub filters: BTreeMap<String, Filter>,

    #[serde(skip)]
    pub name: String,

    #[serde(skip)]
    pub compiled_regex_set: RegexSet,
    #[serde(skip)]
    pub regex_index_to_filter_name: Vec<String>,

    // Plugin-specific
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub stream_type: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub options: Value,
}

impl Stream {
    pub fn get_filter(&self, filter_name: &str) -> Option<&Filter> {
        self.filters.get(filter_name)
    }

    pub fn merge(&mut self, other: Stream) -> Result<(), String> {
        self.cmd = merge_attrs(self.cmd.clone(), other.cmd, Vec::default(), "cmd")?;
        self.stream_type = merge_attrs(self.stream_type.clone(), other.stream_type, None, "type")?;
        self.eof = merge_attrs(self.eof.clone(), other.eof, Eof::default(), "eof")?;

        for (key, filter) in other.filters.into_iter() {
            if self.filters.insert(key.clone(), filter).is_some() {
                return Err(format!(
                    "filter {} is already defined. filter definitions can't be spread accross multiple files.",
                    key
                ));
            }
        }

        Ok(())
    }

    pub fn is_plugin(&self) -> bool {
        self.stream_type
            .as_ref()
            .is_some_and(|stream_type| stream_type != "cmd")
    }

    pub fn setup(&mut self, name: &str, patterns: &Patterns) -> Result<(), String> {
        self._setup(name, patterns)
            .map_err(|msg| format!("stream {}: {}", name, msg))
    }

    fn _setup(&mut self, name: &str, patterns: &Patterns) -> Result<(), String> {
        self.name = name.to_string();

        if self.name.is_empty() {
            return Err("stream name is empty".into());
        }
        if self.name.contains('.') {
            return Err("character '.' is not allowed in stream name".into());
        }

        self.eof.setup()?;

        if !self.is_plugin() {
            if self.cmd.is_empty() {
                return Err("cmd is empty".into());
            }
            if self.cmd[0].is_empty() {
                return Err("cmd's first item is empty".into());
            }
            if !self.options.is_null() {
                return Err("can't define options without a plugin type".into());
            }
        } else if !self.cmd.is_empty() {
            return Err("can't define cmd and a plugin type".into());
        }

        if self.filters.is_empty() {
            return Err("no filters configured".into());
        }

        for (key, filter) in &mut self.filters {
            filter.setup(name, key, patterns)?;
        }

        let all_regexes: BTreeMap<_, _> = self
            .filters
            .values()
            .flat_map(|filter| {
                if let Some(hints) = filter.hint_regexes() {
                    hints
                        .into_iter()
                        .map(|regex| (regex, filter.name.clone()))
                        .collect()
                } else {
                    vec![]
                }
            })
            .collect();

        self.compiled_regex_set = RegexSet::new(all_regexes.keys())
            .map_err(|err| format!("too much regexes on the filters of this stream: {err}"))?;
        self.regex_index_to_filter_name = all_regexes.into_values().collect();

        Ok(())
    }

    pub fn to_stream_config(&self) -> Result<StreamConfig, String> {
        Ok(StreamConfig {
            stream_name: self.name.clone(),
            stream_type: self.stream_type.clone().ok_or_else(|| {
                format!("stream {} doesn't load a plugin. this is a bug!", self.name)
            })?,
            config: self.options.clone().into(),
        })
    }
}

impl PartialEq for Stream {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}
impl Eq for Stream {}
impl Ord for Stream {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}
impl PartialOrd for Stream {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Hash for Stream {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::concepts::filter::tests::ok_filter;

    fn ok_stream() -> Stream {
        Stream {
            cmd: vec!["command".into()],
            filters: BTreeMap::from([("name".into(), ok_filter())]),
            ..Default::default()
        }
    }

    #[test]
    fn test() {
        let mut stream;
        let name = "name";

        // missing cmd
        stream = ok_stream();
        stream.cmd = Vec::new();
        assert!(stream.setup(name, &BTreeMap::new()).is_err());

        // missing cmd
        stream = ok_stream();
        stream.cmd = vec!["".into(), "arg1".into()];
        assert!(stream.setup(name, &BTreeMap::new()).is_err());

        // missing filters
        stream = ok_stream();
        stream.filters = BTreeMap::new();
        assert!(stream.setup(name, &BTreeMap::new()).is_err());

        // stream ok
        stream = ok_stream();
        assert!(stream.setup(name, &BTreeMap::new()).is_ok());
    }

    #[test]
    fn eof() {
        let stream_template = r#"
            "cmd": ["true"],
            "filters": {
                "f1": {
                    "regex": ["^$"],
                    "actions": {
                        "a1": {
                            "cmd": ["true"]
                        }
                    }
                }
            }
        "#;

        // behavior is mandatory
        serde_json::from_str::<Stream>(&format!(
            r#"
                {{
                    "eof": {{}},
                    {stream_template}
                }}
            "#
        ))
        .unwrap_err();

        for error_config in [
            // retry but behavior is not restart
            r#"
                "eof": {
                    "behavior": "ignore",
                    "retry": 2
                }
            "#,
            r#"
                "eof": {
                    "behavior": "exit",
                    "retry": 2
                }
            "#,
            // after but behavior is not restart
            r#"
                "eof": {
                    "behavior": "exit",
                    "after": "10s"
                }
            "#,
            // finally but behavior is not restart
            r#"
                "eof": {
                    "behavior": "exit",
                    "retry": 10,
                    "finally": "exit"
                }
            "#,
            // finally but no retry
            r#"
                "eof": {
                    "behavior": "restart",
                    "finally": "ignore"
                }
            "#,
        ] {
            let mut stream = serde_json::from_str::<Stream>(&format!(
                r#"
                {{
                    {error_config},
                    {stream_template}
                }}
            "#
            ))
            .unwrap();
            stream.setup("s1", &BTreeMap::default()).unwrap_err();
        }

        for ok_config in [
            r#"
                "eof": {
                    "behavior": "ignore"
                }
            "#,
            r#"
                "eof": {
                    "behavior": "exit"
                }
            "#,
            r#"
                "eof": {
                    "behavior": "restart"
                }
            "#,
            r#"
                "eof": {
                    "behavior": "restart",
                    "after": "1min"
                }
            "#,
            r#"
                "eof": {
                    "behavior": "restart",
                    "retry": 1,
                    "finally": "ignore"
                }
            "#,
            r#"
                "eof": {
                    "behavior": "restart",
                    "after": "1min",
                    "retry": 10,
                    "finally": "exit"
                }
            "#,
        ] {
            let mut stream = serde_json::from_str::<Stream>(&format!(
                r#"
                {{
                    {ok_config},
                    {stream_template}
                }}
            "#
            ))
            .unwrap();
            stream.setup("s1", &BTreeMap::default()).unwrap();
        }
    }
}
