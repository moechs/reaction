#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use log::{error, info};
use serde::Deserialize;

use crate::{pattern::Pattern, stream::Stream};

pub type Patterns = BTreeMap<String, Pattern>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    patterns: Patterns,

    streams: BTreeMap<String, Stream>,

    #[serde(default = "num_cpus::get")]
    concurrency: usize,

    #[serde(default)]
    start: Vec<Vec<String>>,
    #[serde(default)]
    stop: Vec<Vec<String>>,
}

impl Config {
    pub fn setup(&mut self) -> Result<()> {
        self._setup()
            .or_else(|msg| Err(anyhow!("Bad configuration: {}", msg)))
    }

    pub fn _setup(&mut self) -> Result<(), String> {
        if self.concurrency == 0 {
            self.concurrency = num_cpus::get();
        }

        for (key, value) in &mut self.patterns {
            value.setup(key)?;
        }

        if self.streams.len() == 0 {
            return Err("no streams configured".into());
        }

        for (key, value) in &mut self.streams {
            value.setup(key, &self.patterns)?;
        }
        Ok(())
    }
}

enum Format {
    YAML,
    JSON,
    JSONnet,
}

pub fn config_from_file(path: &PathBuf) -> Result<Config> {
    _config_from_file(path).with_context(|| anyhow!("Configuration file {}:", path.display()))
}
fn _config_from_file(path: &PathBuf) -> Result<Config> {
    let extension = path
        .extension()
        .map(|ex| ex.to_str())
        .flatten()
        .ok_or(anyhow!("no file extension"))?;

    let format = match extension {
        "yaml" | "yml" => Format::YAML,
        "json" => Format::JSON,
        "jsonnet" => Format::JSONnet,
        _ => {
            return Err(anyhow!(
                "extension {} is not recognized. Must be json, jsonnet, yml or yaml.",
                extension
            ))
        }
    };

    let mut config: Config = match format {
        Format::JSON => serde_json::from_reader(File::open(&path)?)?,
        Format::YAML => serde_yaml::from_reader(File::open(&path)?)?,
        Format::JSONnet => serde_json::from_str(&jsonnet::from_path(&path)?)?,
    };

    config.setup()?;

    return Ok(config);
}

mod jsonnet {
    use std::path::PathBuf;

    use anyhow::{anyhow, Result};
    use jrsonnet_evaluator::{error::LocError, EvaluationState, FileImportResolver};

    pub fn from_path(path: &PathBuf) -> Result<String> {
        let state = EvaluationState::default();
        state.with_stdlib();
        state.set_import_resolver(Box::new(FileImportResolver::default()));
        // state.set_import_resolver(Box::new(FileImportResolver {
        //     library_paths: Vec::new(),
        // }));

        match evaluate(path, &state) {
            Ok(val) => Ok(val),
            Err(err) => Err(anyhow!("{}", state.stringify_err(&err))),
        }
    }
    fn evaluate(path: &PathBuf, state: &EvaluationState) -> Result<String, LocError> {
        let val = state.evaluate_file_raw(path)?;
        let result = state.manifest(val)?;
        Ok(result.to_string())
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn default_config() -> Config {
        Config {
            concurrency: 0,
            patterns: BTreeMap::new(),
            streams: BTreeMap::new(),
            start: Vec::new(),
            stop: Vec::new(),
        }
    }

    #[test]
    fn config_missing() {
        let mut config = default_config();
        assert!(config.setup().is_err());
    }
}
