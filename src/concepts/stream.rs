use std::{cmp::Ordering, collections::BTreeMap, hash::Hash};

use reaction_plugin::StreamConfig;
use regex::RegexSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Filter, Patterns, merge_attrs};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(Default))]
#[serde(deny_unknown_fields)]
pub struct Stream {
    #[serde(default)]
    pub cmd: Vec<String>,

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
                filter
                    .hint_regexes()
                    .into_iter()
                    .map(|regex| (regex, filter.name.clone()))
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
}
