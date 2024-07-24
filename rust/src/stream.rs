use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{config::Patterns, filter::Filter};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    cmd: Vec<String>,
    filters: BTreeMap<String, Filter>,

    #[serde(skip)]
    name: String,
}

impl Stream {
    pub fn setup(&mut self, name: &String, patterns: &Patterns) -> Result<(), String> {
        self._setup(name, patterns)
            .or_else(|msg| Err(format!("stream {}: {}", name, msg)))
    }

    fn _setup(&mut self, name: &String, patterns: &Patterns) -> Result<(), String> {
        self.name = name.clone();

        if self.name.len() == 0 {
            return Err("stream name is empty".into());
        }
        if self.name.contains('.') {
            return Err("character '.' is not allowed in stream name".into());
        }

        if self.cmd.len() == 0 {
            return Err("cmd is empty".into());
        }
        if self.cmd[0].len() == 0 {
            return Err("cmd's first item is empty".into());
        }

        if self.filters.len() == 0 {
            return Err("no filters configured".into());
        }

        for (key, value) in &mut self.filters {
            value.setup(&name, key, patterns)?;
        }

        Ok(())
    }
}

#[cfg(test)]
pub mod tests {

    use super::*;

    fn default_stream() -> Stream {
        Stream {
            cmd: Vec::new(),
            name: "".into(),
            filters: BTreeMap::new(),
        }
    }

    pub fn ok_stream() -> Stream {
        let mut stream = default_stream();
        stream.cmd = vec!["command".into()];
        stream
            .filters
            .insert("name".into(), crate::filter::tests::ok_filter());
        stream
    }

    #[test]
    fn test() {
        let mut stream;
        let name = "name".into();

        // missing cmd
        stream = ok_stream();
        stream.cmd = Vec::new();
        assert!(stream.setup(&name, &BTreeMap::new()).is_err());

        // missing cmd
        stream = ok_stream();
        stream.cmd = vec!["".into(), "arg1".into()];
        assert!(stream.setup(&name, &BTreeMap::new()).is_err());

        // missing filters
        stream = ok_stream();
        stream.filters = BTreeMap::new();
        assert!(stream.setup(&name, &BTreeMap::new()).is_err());

        // stream ok
        stream = ok_stream();
        assert!(stream.setup(&name, &BTreeMap::new()).is_ok());
    }
}
