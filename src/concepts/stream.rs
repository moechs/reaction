use std::{cmp::Ordering, collections::BTreeMap, hash::Hash};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{merge_attrs, null_value, Filter, Patterns};

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

    // Plugin-specific
    #[serde(default, rename = "type")]
    pub stream_type: Option<String>,
    #[serde(default = "null_value")]
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
                return Err(format!("filter {} is already defined. filter definitions can't be spread accross multiple files.", key));
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
        } else {
            if !self.cmd.is_empty() {
                return Err("can't define cmd and a plugin type".into());
            }
        }

        if self.filters.is_empty() {
            return Err("no filters configured".into());
        }

        for (key, filter) in &mut self.filters {
            filter.setup(name, key, patterns)?;
        }

        Ok(())
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
        let mut stream = Stream::default();
        stream.cmd = vec!["command".into()];
        stream.filters.insert("name".into(), ok_filter());
        stream
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
