use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{config::Patterns, filter::Filter};

#[derive(Debug, Deserialize)]
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

        if self.name.contains('.') {
            return Err("character '.' is not allowed in stream name".into());
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

