use std::{
    collections::{btree_map::Entry, BTreeMap},
    fs::File,
    io,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error, info, warn};

use super::{Filter, Pattern, Stream};

pub type Patterns = BTreeMap<String, Arc<Pattern>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(Default))]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "num_cpus::get")]
    concurrency: usize,
    #[serde(default = "dot", skip_serializing_if = "String::is_empty")]
    state_directory: String,

    #[serde(default)]
    patterns: Patterns,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    start: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    stop: Vec<Vec<String>>,

    #[serde(default)]
    streams: BTreeMap<String, Stream>,

    // This field only serve the purpose of having a top-level place for saving YAML variables
    #[serde(default, skip_serializing, rename = "definitions")]
    _definitions: serde_json::Value,
}

fn dot() -> String {
    ".".into()
}

impl Config {
    pub fn streams(&self) -> &BTreeMap<String, Stream> {
        &self.streams
    }

    pub fn patterns(&self) -> &Patterns {
        &self.patterns
    }

    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    pub fn state_directory(&self) -> &str {
        &self.state_directory
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

    fn merge(&mut self, mut other: Config) -> Result<(), String> {
        for (key, pattern) in other.patterns.into_iter() {
            match self.patterns.entry(key) {
                Entry::Vacant(e) => {
                    e.insert(pattern);
                }
                Entry::Occupied(e) => {
                    return Err(format!("pattern {} is already defined. pattern definitions can't be spread accross multiple files.", e.key()));
                }
            }
        }

        for (key, stream) in other.streams.into_iter() {
            match self.streams.entry(key) {
                Entry::Vacant(e) => {
                    e.insert(stream);
                }
                Entry::Occupied(mut e) => {
                    e.get_mut()
                        .merge(stream)
                        .map_err(|err| format!("Stream {}: {}", e.key(), err))?;
                }
            }
        }

        self.start.append(&mut other.start);
        self.stop.append(&mut other.stop);

        self.state_directory = other.state_directory;

        Ok(())
    }

    pub fn setup(&mut self) -> Result<(), String> {
        if self.concurrency == 0 {
            self.concurrency = num_cpus::get();
        }

        // Nullify this useless field
        self._definitions = serde_json::Value::Null;

        if self.patterns.is_empty() {
            return Err("no patterns configured".into());
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

    pub fn from_path(path: &Path) -> Result<Self, String> {
        match Self::from_path_raw(path) {
            Ok((mut cfg, files)) => {
                cfg.setup().map_err(ConfigError::BadConfig).map_err(|e| {
                    if path.is_dir() {
                        format!(
                        "{e}\nWhile reading config from {}. List of files read, in that order:\n{}",
                        path.display(),
                        files.join("\n"),
                    )
                    } else {
                        format!("{e}\nWhile reading config from {}.", path.display())
                    }
                })?;
                Ok(cfg)
            }
            Err(e) => Err(e),
        }
    }

    pub fn from_path_raw(path: &Path) -> Result<(Self, Vec<String>), String> {
        match std::fs::metadata(path) {
            Err(e) => Err(format!("Error accessing {}: {e}", path.to_string_lossy())),
            Ok(m) => {
                if m.is_file() {
                    Self::_from_file_raw(path)
                        .map(|cfg| {
                            let fname = path
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or("".to_string());
                            (cfg, vec![fname])
                        })
                        .map_err(|e| format!("Configuration file {}: {}", path.display(), e))
                } else if m.is_dir() {
                    Self::_from_dir_raw(path)
                } else {
                    Err(format!(
                        "Invalid file type for {}: not a file nor a directory",
                        path.to_string_lossy()
                    ))
                }
            }
        }
    }

    fn _from_dir_raw(path: &Path) -> Result<(Self, Vec<String>), String> {
        let dir = std::fs::read_dir(path)
            .map_err(|e| format!("Error accessing directory {}: {e}", path.display()))?;
        let mut cfg: Option<Self> = None;
        let mut read_cfg_fname = vec![];
        for f in dir {
            let f =
                f.map_err(|e| format!("Error while reading directory {}: {e}", path.display()))?;

            let fname = f.file_name();
            let fname = match fname.to_str() {
                Some(fname) => fname,
                None => {
                    warn!(
                        "Ignoring file {} in {}",
                        f.file_name().to_string_lossy(),
                        path.display()
                    );
                    continue;
                }
            };

            if fname.starts_with(".") || fname.starts_with("_") {
                // silently ignore hidden file
                debug!("Ignoring hidden file {fname} in {}", path.display());
                continue;
            }

            let fpath = f.path();
            let ext = match fpath.extension() {
                None => {
                    // silently ignore files without extensions (may be directory)
                    debug!(
                        "Ignoring file without extension {fname} in {}",
                        path.display()
                    );
                    continue;
                }
                Some(ext) => {
                    if let Some(ext) = ext.to_str() {
                        ext
                    } else {
                        warn!(
                            "Ignoring file {} in {} with unexpected extension",
                            fname,
                            path.display()
                        );
                        continue;
                    }
                }
            };

            let cfg_format = match Self::_extension_to_format(ext) {
                Ok(fmt) => fmt,
                Err(_) => {
                    // silently ignore files without an expected extension
                    debug!(
                        "Ignoring file with non recognized extension {fname} in {}",
                        path.display()
                    );
                    continue;
                }
            };

            let cfg_part = Self::_load_file(&fpath, cfg_format)
                .map_err(|e| format!("While reading {fname} in {}: {e}", path.display()))?;
            read_cfg_fname.push(fname.to_string());

            if let Some(mut cfg_agg) = cfg.take() {
                cfg_agg.merge(cfg_part)?;
                cfg = Some(cfg_agg);
            } else {
                cfg = Some(cfg_part)
            }
        }

        if let Some(cfg) = cfg {
            Ok((cfg, read_cfg_fname))
        } else {
            Err(format!(
                "No valid configuration files found in {}",
                path.display()
            ))
        }
    }

    fn _extension_to_format(extension: &str) -> Result<Format, ConfigError> {
        match extension {
            "yaml" | "yml" => Ok(Format::Yaml),
            "json" => Ok(Format::Json),
            "jsonnet" => Ok(Format::Jsonnet),
            _ => Err(ConfigError::Extension(format!(
                "extension {} is not recognized",
                extension
            ))),
        }
    }

    fn _load_file(path: &Path, format: Format) -> Result<Self, ConfigError> {
        let cfg: Self = match format {
            Format::Json => serde_json::from_reader(File::open(path)?)?,
            Format::Yaml => serde_yaml::from_reader(File::open(path)?)?,
            Format::Jsonnet => serde_json::from_str(&jsonnet::from_path(path)?)?,
        };
        Ok(cfg)
    }

    fn _from_file_raw(path: &Path) -> Result<Self, ConfigError> {
        let extension = path
            .extension()
            .and_then(|ex| ex.to_str())
            .ok_or(ConfigError::Extension("no file extension".into()))?;

        let format = Self::_extension_to_format(extension)?;

        Self::_load_file(path, format)
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
#[allow(clippy::unwrap_used)]
mod tests {

    use super::*;

    #[test]
    fn config_missing() {
        let mut config = Config::default();
        assert!(config.setup().is_err());
    }

    fn parse_config_json(cfg: &str) -> Config {
        const DUMMY_PATTERNS: &str = r#"
        "patterns": {
            "zero": {
                "regex": "0"
            }
        }"#;
        const DUMMY_FILTERS: &str = r#"
        "filters": {
            "dummy": {
                "regex": ["abc"],
                "actions": {
                    "act": {
                        "cmd": ["echo", "1"]
                    }
                }
            }
        }"#;
        const DUMMY_STREAMS: &str = r#"
        "streams": {
            "dummy": {
                "cmd": ["true"],
                {{FILTERS}}
            }
        }
        "#;
        let cfg = cfg
            .to_string()
            .replace("{{STREAMS}}", DUMMY_STREAMS)
            .replace("{{PATTERNS}}", DUMMY_PATTERNS)
            .replace("{{FILTERS}}", DUMMY_FILTERS);

        serde_json::from_str(&cfg)
            .inspect_err(|_| {
                eprintln!("{cfg}");
            })
            .unwrap()
    }

    #[test]
    fn config_without_stream() {
        let mut cfg1 = parse_config_json(
            r#"{
            {{PATTERNS}}
        }"#,
        );
        let mut cfg2 = parse_config_json(
            r#"{
            {{PATTERNS}},
            "streams": {}
        }"#,
        );

        let res = cfg1.setup();
        assert!(res.is_err());
        let res = cfg2.setup();
        assert!(res.is_err());
    }

    #[test]
    fn config_without_pattern() {
        let mut cfg1 = parse_config_json(
            r#"{
            {{STREAMS}}
        }"#,
        );
        let mut cfg2 = parse_config_json(
            r#"{
            "patterns": {},
            {{STREAMS}}
        }"#,
        );

        let res = cfg1.setup();
        assert!(res.is_err());
        let res = cfg2.setup();
        assert!(res.is_err());
    }

    #[test]
    fn merge_config_distinct_patterns() {
        let mut cfg_org = parse_config_json(
            r#"{
                "patterns": {
                    "ip4": {
                        "regex": "ip4"
                    }
                },
                {{STREAMS}}
            }"#,
        );
        let cfg_oth = parse_config_json(
            r#"{
                "patterns": {
                    "ip6": {
                        "regex": "ip6"
                    }
                }
            }"#,
        );

        cfg_org.merge(cfg_oth).unwrap();
        cfg_org.setup().unwrap();
        assert!(cfg_org.patterns.contains_key("ip4"));
        assert!(cfg_org.patterns.contains_key("ip6"));
        assert_eq!(cfg_org.patterns.len(), 2);
        assert!(cfg_org.streams.contains_key("dummy"));
        assert_eq!(cfg_org.streams.len(), 1);
    }

    #[test]
    fn merge_config_same_patterns() {
        let mut cfg_org = parse_config_json(
            r#"{
                "patterns": {
                    "zero": {
                        "regex": "0"
                    }
                },
                {{STREAMS}}
            }"#,
        );
        let cfg_oth = parse_config_json(
            r#"{
                "patterns": {
                    "zero": {
                        "regex": "00"
                    }
                }
            }"#,
        );

        let res = cfg_org.merge(cfg_oth);
        assert!(res.is_err());
    }

    #[test]
    fn merge_config_distinct_streams() {
        let mut cfg_org = parse_config_json(
            r#"{
                {{PATTERNS}},
                "streams": {
                    "echo1": {
                        "cmd": ["echo"],
                        {{FILTERS}}
                    }
                }
            }"#,
        );
        let cfg_oth = parse_config_json(
            r#"{
                "streams": {
                    "echo2": {
                        "cmd": ["echo"],
                        {{FILTERS}}
                    }
                }
            }"#,
        );

        cfg_org.merge(cfg_oth).unwrap();
        cfg_org.setup().unwrap();
        assert!(cfg_org.patterns.contains_key("zero"));
        assert_eq!(cfg_org.patterns.len(), 1);
        assert!(cfg_org.streams.contains_key("echo1"));
        assert!(cfg_org.streams.contains_key("echo2"));
        assert_eq!(cfg_org.streams.len(), 2);
    }

    #[test]
    fn merge_config_same_streams_distinct_filters() {
        let mut cfg_org = parse_config_json(
            r#"{
                {{PATTERNS}},
                "streams": {
                    "echo": {
                        "cmd": ["echo"],
                        "filters": {
                            "f1": {
                                "regex": ["abc"],
                                "actions": {
                                    "act": {
                                        "cmd": ["echo", "1"]
                                    }
                                }
                            }
                        }
                    }
                }
            }"#,
        );
        let cfg_oth = parse_config_json(
            r#"{
                "streams": {
                    "echo": {
                        "cmd": ["echo"],
                        "filters": {
                            "f2": {
                                "regex": ["abc"],
                                "actions": {
                                    "act": {
                                        "cmd": ["echo", "1"]
                                    }
                                }
                            }
                        }
                    }
                }
            }"#,
        );

        cfg_org.merge(cfg_oth).unwrap();
        cfg_org.setup().unwrap();
        assert!(cfg_org.streams.contains_key("echo"));
        assert_eq!(cfg_org.streams.len(), 1);

        let filters = cfg_org.streams.get("echo").unwrap().filters();
        assert!(filters.contains_key("f1"));
        assert!(filters.contains_key("f2"));
        assert_eq!(filters.len(), 2);
    }

    #[test]
    fn merge_config_same_streams_distinct_command() {
        let mut cfg_org = parse_config_json(
            r#"{
                {{PATTERNS}},
                "streams": {
                    "echo": {
                        "cmd": ["echo"],
                        {{FILTERS}}
                    }
                }
            }"#,
        );
        let cfg_oth = parse_config_json(
            r#"{
                "streams": {
                    "echo": {
                        "cmd": ["true"]
                    }
                }
            }"#,
        );

        let res = cfg_org.merge(cfg_oth);
        assert!(res.is_err());
    }

    #[test]
    fn merge_config_same_streams_command_in_one() {
        let mut cfg_org = parse_config_json(
            r#"{
                {{PATTERNS}},
                "streams": {
                    "echo": {
                        {{FILTERS}}
                    }
                }
            }"#,
        );
        let cfg_oth = parse_config_json(
            r#"{
                "streams": {
                    "echo": {
                        "cmd": ["echo"]
                    }
                }
            }"#,
        );

        cfg_org.merge(cfg_oth).unwrap();
        cfg_org.setup().unwrap();
        assert!(cfg_org.streams.contains_key("echo"));
        assert_eq!(cfg_org.streams.len(), 1);
        let stream = cfg_org.streams.get("echo").unwrap();
        assert_eq!(stream.cmd().len(), 1);
        assert_eq!(stream.filters().len(), 1);
    }

    #[test]
    fn merge_config_same_streams_same_filters() {
        let mut cfg_org = parse_config_json(
            r#"{
                {{PATTERNS}},
                "streams": {
                    "echo": {
                        "cmd": ["echo"],
                        {{FILTERS}}
                    }
                }
            }"#,
        );
        let cfg_oth = parse_config_json(
            r#"{
                "streams": {
                    "echo": {
                        "cmd": ["echo"],
                        {{FILTERS}}
                    }
                }
            }"#,
        );

        let res = cfg_org.merge(cfg_oth);
        assert!(res.is_err());
    }
}
