use std::{
    collections::BTreeMap,
    fs::File,
    io,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
};

use log::{error, info};
use serde::Deserialize;
use thiserror::Error;

use super::{Filter, Pattern, Stream};

pub type Patterns = BTreeMap<String, Arc<Pattern>>;

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

    pub fn filters(&self) -> Vec<&Filter> {
        self.streams
            .values()
            .flat_map(|stream| stream.filters().values())
            .collect()
    }

    pub fn get_filter(&self, name: &(String, String)) -> Option<&Filter> {
        self.streams
            .get(&name.0)
            .and_then(|stream| stream.get_filter(&name.1))
    }

    pub fn setup(&mut self) -> Result<(), String> {
        if self.concurrency == 0 {
            self.concurrency = num_cpus::get();
        }

        let mut new_patterns = BTreeMap::new();
        for (key, value) in &self.patterns {
            let mut value = value.as_ref().clone();
            value.setup(key)?;
            new_patterns.insert(key.clone(), Arc::new(value));
        }
        self.patterns = new_patterns;

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

    pub fn from_file(path: &Path) -> Result<Self, String> {
        Config::_from_file(path)
            .map_err(|err| format!("Configuration file {}: {}", path.display(), err))
    }

    fn _from_file(path: &Path) -> Result<Self, ConfigError> {
        let extension = path
            .extension()
            .and_then(|ex| ex.to_str())
            .ok_or(ConfigError::Extension("no file extension".into()))?;

        let format = match extension {
            "yaml" | "yml" => Ok(Format::Yaml),
            "json" => Ok(Format::Json),
            "jsonnet" => Ok(Format::Jsonnet),
            _ => Err(ConfigError::Extension(format!(
                "extension {} is not recognized",
                extension
            ))),
        }?;

        let mut config: Config = match format {
            Format::Json => serde_json::from_reader(File::open(path)?)?,
            Format::Yaml => serde_yaml::from_reader(File::open(path)?)?,
            Format::Jsonnet => serde_json::from_str(&jsonnet::from_path(path)?)?,
        };

        config.setup().map_err(ConfigError::BadConfig)?;

        Ok(config)
    }
}

enum Format {
    Yaml,
    Json,
    Jsonnet,
}

#[derive(Error, Debug)]
enum ConfigError {
    #[error("{0}")]
    Any(String),
    #[error("Bad configuration: {0}")]
    BadConfig(String),
    #[error("{0}. Must be json, jsonnet, yml or yaml.")]
    Extension(String),
    #[error("{0}")]
    IO(#[from] io::Error),
    #[error("{0}")]
    JSON(#[from] serde_json::Error),
    #[error("{0}")]
    YAML(#[from] serde_yaml::Error),
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
            Err(err) => Err(ConfigError::Any(state.stringify_err(&err))),
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
