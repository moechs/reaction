use std::{
    collections::BTreeMap,
    fmt::Display,
    fs::File,
    io,
    path::Path,
    process::{Command, Stdio},
};

use log::{error, info};
use serde::Deserialize;

use crate::{
    filter::{Filter, FilterName},
    pattern::Pattern,
    stream::Stream,
};

#[derive(Debug)]
pub struct ConfigError {
    err: String,
}

impl From<String> for ConfigError {
    fn from(value: String) -> Self {
        ConfigError { err: value }
    }
}

impl From<&str> for ConfigError {
    fn from(value: &str) -> Self {
        ConfigError {
            err: value.to_string(),
        }
    }
}

impl From<io::Error> for ConfigError {
    fn from(value: io::Error) -> Self {
        ConfigError {
            err: value.to_string(),
        }
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(value: serde_json::Error) -> Self {
        ConfigError {
            err: value.to_string(),
        }
    }
}

impl From<serde_yaml::Error> for ConfigError {
    fn from(value: serde_yaml::Error) -> Self {
        ConfigError {
            err: value.to_string(),
        }
    }
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.err)
    }
}

pub type Patterns = BTreeMap<String, Pattern>;

#[derive(Clone, Debug, Deserialize)]
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
    pub fn streams(&self) -> &BTreeMap<String, Stream> {
        &self.streams
    }

    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    pub fn get_filter(&self, stream: &str, filter: &str) -> Option<&Filter> {
        self.streams.get(stream)?.filters().get(filter)
    }

    pub fn filter_names(&self) -> Vec<FilterName> {
        self.streams
            .values()
            .flat_map(|stream| stream.filters().values().map(|filter| filter.name()))
            .collect()
    }

    pub fn setup(&mut self) -> Result<(), ConfigError> {
        Ok(self
            ._setup()
            .map_err(|msg| format!("Bad configuration: {}", msg))?)
    }

    pub fn _setup(&mut self) -> Result<(), String> {
        if self.concurrency == 0 {
            self.concurrency = num_cpus::get();
        }

        for (key, value) in &mut self.patterns {
            value.setup(key)?;
        }

        if self.streams.is_empty() {
            return Err("no streams configured".into());
        }

        for (key, value) in &mut self.streams {
            value.setup(key, &self.patterns)?;
        }
        Ok(())
    }

    pub fn start(&self) -> bool {
        run_commands(&self.start, "start")
    }

    pub fn stop(&self) -> bool {
        run_commands(&self.stop, "stop")
    }
}

enum Format {
    Yaml,
    Json,
    Jsonnet,
}
pub fn config_from_file(path: &Path) -> Result<Config, ConfigError> {
    Ok(_config_from_file(path)
        .map_err(|msg| format!("Configuration file {}: {}", path.display(), msg.err))?)
}
fn _config_from_file(path: &Path) -> Result<Config, ConfigError> {
    let extension = path
        .extension()
        .and_then(|ex| ex.to_str())
        .ok_or("no file extension")?;

    let format = match extension {
        "yaml" | "yml" => Format::Yaml,
        "json" => Format::Json,
        "jsonnet" => Format::Jsonnet,
        _ => {
            return Err(format!(
                "extension {} is not recognized. Must be json, jsonnet, yml or yaml.",
                extension
            )
            .into())
        }
    };

    let mut config: Config = match format {
        Format::Json => serde_json::from_reader(File::open(path)?)?,
        Format::Yaml => serde_yaml::from_reader(File::open(path)?)?,
        Format::Jsonnet => serde_json::from_str(&jsonnet::from_path(path)?)?,
    };

    config.setup()?;

    Ok(config)
}

mod jsonnet {
    use std::path::Path;

    use jrsonnet_evaluator::{error::LocError, EvaluationState, FileImportResolver};

    use super::ConfigError;

    pub fn from_path(path: &Path) -> Result<String, ConfigError> {
        let state = EvaluationState::default();
        state.with_stdlib();
        state.set_import_resolver(Box::<FileImportResolver>::default());
        // state.set_import_resolver(Box::new(FileImportResolver {
        //     library_paths: Vec::new(),
        // }));

        match evaluate(path, &state) {
            Ok(val) => Ok(val),
            Err(err) => Err(state.stringify_err(&err).into()),
        }
    }
    fn evaluate(path: &Path, state: &EvaluationState) -> Result<String, LocError> {
        let val = state.evaluate_file_raw(path)?;
        let result = state.manifest(val)?;
        Ok(result.to_string())
    }
}

fn run_commands(commands: &Vec<Vec<String>>, moment: &str) -> bool {
    let mut ok = true;
    for command in commands {
        info!("{} command: run {:?}\n", moment, command);
        // TODO reaction-go waits the subprocess completion for a minute maximum
        match Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(exit_status) => {
                if !exit_status.success() {
                    error!(
                        "{} command: run {:?}: exit code: {}",
                        moment,
                        command,
                        exit_status.code().unwrap_or(1)
                    );
                    ok = false;
                }
            }
            Err(err) => {
                error!(
                    "{} command: run {:?}: could not execute: {}",
                    moment, command, err
                );
                ok = false;
            }
        }
    }

    ok
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
