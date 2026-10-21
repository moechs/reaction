use std::{
    collections::{BTreeMap, btree_map::Entry},
    fs::File,
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

use reaction_plugin::time::{deser_opt_duration, print_duration, ser_opt_duration};
use serde::{Deserialize, Deserializer, Serialize, de};
use thiserror::Error;
use tracing::{Level, debug, error, info, warn};

use crate::cli::parse_log_level;

use super::{Pattern, Plugin, Stream, merge_attrs};

#[cfg(not(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "illumos"
)))]
const DEFAULT_PATHS: [&str; 1] = ["/etc/reaction"];
#[cfg(any(
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "illumos"
))]
const DEFAULT_PATHS: [&str; 2] = ["/etc/reaction", "/usr/local/etc/reaction"];

pub type Patterns = BTreeMap<String, Arc<Pattern>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(Default))]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "num_cpus::get")]
    pub concurrency: usize,
    #[serde(
        default,
        skip_serializing_if = "level_is_info",
        deserialize_with = "level_deserialize"
    )]
    pub log_level: LogLevel,

    #[serde(
        default = "default_runtime_dir",
        skip_serializing_if = "is_default_runtime_dir"
    )]
    pub runtime_directory: PathBuf,
    #[serde(
        default = "default_state_dir",
        skip_serializing_if = "is_default_state_dir"
    )]
    pub state_directory: PathBuf,

    #[serde(
        default = "one_day",
        skip_serializing_if = "is_one_day",
        deserialize_with = "deser_opt_duration",
        serialize_with = "ser_opt_duration"
    )]
    pub state_rotation_interval: Option<Duration>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins: BTreeMap<String, Plugin>,

    #[serde(default)]
    pub patterns: Patterns,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub start: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<Vec<String>>,

    #[serde(default)]
    pub streams: BTreeMap<String, Stream>,

    // This field only serve the purpose of having a top-level place for saving YAML variables
    #[serde(default, skip_serializing, rename = "definitions")]
    _definitions: serde_json::Value,
}

fn default_runtime_dir() -> PathBuf {
    "/run/reaction/".into()
}
fn is_default_runtime_dir(path: &PathBuf) -> bool {
    path == &default_runtime_dir()
}
fn default_state_dir() -> PathBuf {
    "/var/lib/reaction/".into()
}
fn is_default_state_dir(path: &PathBuf) -> bool {
    path == &default_state_dir()
}

fn one_day() -> Option<Duration> {
    Some(Duration::from_hours(24))
}
fn is_one_day(interval: &Option<Duration>) -> bool {
    interval == &one_day()
}

#[derive(Clone, PartialEq, Eq)]
pub struct LogLevel(pub Level);

impl Default for LogLevel {
    fn default() -> Self {
        Self(Level::INFO)
    }
}
impl std::fmt::Debug for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl Serialize for LogLevel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}
fn level_is_info(level: &LogLevel) -> bool {
    level.0 == Level::INFO
}
fn level_deserialize<'de, D>(deserializer: D) -> Result<LogLevel, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?.to_ascii_uppercase();
    Ok(parse_log_level(&s)
        .map(LogLevel)
        .map_err(de::Error::custom)?)
}

impl Config {
    fn merge(&mut self, mut other: Config) -> Result<(), String> {
        for (key, plugin) in other.plugins.into_iter() {
            match self.plugins.entry(key) {
                Entry::Vacant(e) => {
                    e.insert(plugin);
                }
                Entry::Occupied(mut e) => {
                    e.get_mut()
                        .merge(plugin)
                        .map_err(|err| format!("Plugin {}: {}", e.key(), err))?;
                }
            }
        }

        for (key, pattern) in other.patterns.into_iter() {
            match self.patterns.entry(key) {
                Entry::Vacant(e) => {
                    e.insert(pattern);
                }
                Entry::Occupied(e) => {
                    return Err(format!(
                        "pattern {} is already defined. pattern definitions can't be spread accross multiple files.",
                        e.key()
                    ));
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

        self.runtime_directory = merge_attrs(
            self.runtime_directory.clone(),
            other.runtime_directory,
            default_runtime_dir(),
            "runtime_directory",
        )?;

        self.state_directory = merge_attrs(
            self.state_directory.clone(),
            other.state_directory,
            default_state_dir(),
            "state_directory",
        )?;

        self.state_rotation_interval = merge_attrs(
            self.state_rotation_interval.clone(),
            other.state_rotation_interval,
            one_day(),
            "state_rotation_interval",
        )?;

        self.concurrency = merge_attrs(
            self.concurrency,
            other.concurrency,
            num_cpus::get(),
            "concurrency",
        )?;

        self.log_level = merge_attrs(
            self.log_level.clone(),
            other.log_level,
            LogLevel::default(),
            "log_level",
        )?;

        Ok(())
    }

    pub fn setup(&mut self) -> Result<(), String> {
        if self.concurrency == 0 {
            self.concurrency = num_cpus::get();
        }

        if let Some(interval) = self.state_rotation_interval.as_ref() {
            if interval < &Duration::from_mins(30) {
                return Err(format!(
                    "state_rotation_interval minimum value is set to 30 mins, but you set it to '{}'",
                    print_duration(interval)
                ));
            }
        }

        // Nullify this useless field
        self._definitions = serde_json::Value::Null;

        for (key, value) in &mut self.plugins {
            value.setup(key)?;
        }

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

    pub fn from_path(path: &Option<PathBuf>) -> Result<Self, String> {
        match Self::from_path_raw(path) {
            Ok((mut cfg, files, path)) => {
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

    pub fn from_path_raw(path: &Option<PathBuf>) -> Result<(Self, Vec<String>, PathBuf), String> {
        let path = if let Some(path) = path {
            path.clone()
        } else if let Some(path) = DEFAULT_PATHS
            .iter()
            .find(|default_path| std::fs::metadata(default_path).is_ok_and(|meta| meta.is_dir()))
            .map(|p| PathBuf::from(p))
        {
            path
        } else {
            return Err(format!(
                "-c/--config option is not set, and no config can be found at the default paths: {:?}",
                DEFAULT_PATHS
            ));
        };

        match std::fs::metadata(&path) {
            Err(e) => Err(format!("Error accessing {}: {e}", path.to_string_lossy())),
            Ok(m) => {
                if m.is_file() {
                    Self::_from_file_raw(&path)
                        .map_err(|e| format!("Configuration file {}: {}", path.display(), e))
                        .map(|cfg| {
                            let fname = path
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or("".to_string());
                            (cfg, vec![fname], path)
                        })
                } else if m.is_dir() {
                    Self::_from_dir_raw(&path).map(|res| (res.0, res.1, path))
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
        // sorts files by name
        let mut cfg_files = BTreeMap::new();
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

            match Self::_extension_to_format(ext) {
                Ok(fmt) => cfg_files.insert(fname.to_string(), (fpath, fmt)),
                Err(_) => {
                    // silently ignore files without an expected extension
                    debug!(
                        "Ignoring file with non recognized extension {fname} in {}",
                        path.display()
                    );
                    continue;
                }
            };
        }

        let mut cfg: Option<Self> = None;
        for (fname, (fpath, fmt)) in &cfg_files {
            let cfg_part = Self::_load_file(fpath, *fmt)
                .map_err(|e| format!("While reading {fname} in {}: {e}", path.display()))?;

            if let Some(mut cfg_agg) = cfg.take() {
                cfg_agg.merge(cfg_part)?;
                cfg = Some(cfg_agg);
            } else {
                cfg = Some(cfg_part)
            }
        }

        if let Some(cfg) = cfg {
            Ok((cfg, cfg_files.into_keys().collect()))
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
            Format::Yaml => serde_saphyr::from_reader(File::open(path)?)?,
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

#[derive(Clone, Copy)]
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
    YAML(#[from] serde_saphyr::Error),
}

mod jsonnet {
    use std::path::Path;

    use jrsonnet_evaluator::{
        FileImportResolver, State, StateBuilder, error::Error, manifest::JsonFormat,
        trace::PathResolver,
    };

    use super::ConfigError;

    pub fn from_path(path: &Path) -> Result<String, ConfigError> {
        let mut state_builder = StateBuilder::default();
        state_builder.import_resolver(FileImportResolver::new(match path.parent() {
            Some(dir) => vec![dir.into()],
            None => vec![],
        }));
        state_builder.context_initializer(jrsonnet_stdlib::ContextInitializer::new(
            PathResolver::Relative(path.to_owned()),
        ));
        let state = state_builder.build();
        let _entered = state.enter();

        match evaluate(path, &state) {
            Ok(val) => Ok(val),
            Err(err) => Err(ConfigError::Any(err.to_string())),
        }
    }
    fn evaluate(path: &Path, state: &State) -> Result<String, Error> {
        let val = state.import(path)?;
        let result = val.manifest(JsonFormat::default())?;
        Ok(result.to_string())
    }
}

fn run_commands(commands: &Vec<Vec<String>>, moment: &str) -> bool {
    debug!("Running {moment} commands...");
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

    fn parse_config_json(cfg: &str) -> Result<Config, serde_json::Error> {
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

        serde_json::from_str(&cfg).inspect_err(|_| {
            eprintln!("{cfg}");
        })
    }

    #[test]
    fn config_without_stream() {
        let mut cfg1 = parse_config_json(
            r#"{
            {{PATTERNS}}
        }"#,
        )
        .unwrap();
        let mut cfg2 = parse_config_json(
            r#"{
            {{PATTERNS}},
            "streams": {}
        }"#,
        )
        .unwrap();

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
        )
        .unwrap();
        let mut cfg2 = parse_config_json(
            r#"{
            "patterns": {},
            {{STREAMS}}
        }"#,
        )
        .unwrap();

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
        )
        .unwrap();
        let cfg_oth = parse_config_json(
            r#"{
                "patterns": {
                    "ip6": {
                        "regex": "ip6"
                    }
                }
            }"#,
        )
        .unwrap();

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
        )
        .unwrap();
        let cfg_oth = parse_config_json(
            r#"{
                "patterns": {
                    "zero": {
                        "regex": "00"
                    }
                }
            }"#,
        )
        .unwrap();

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
        )
        .unwrap();
        let cfg_oth = parse_config_json(
            r#"{
                "streams": {
                    "echo2": {
                        "cmd": ["echo"],
                        {{FILTERS}}
                    }
                }
            }"#,
        )
        .unwrap();

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
        )
        .unwrap();
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
        )
        .unwrap();

        cfg_org.merge(cfg_oth).unwrap();
        cfg_org.setup().unwrap();
        assert!(cfg_org.streams.contains_key("echo"));
        assert_eq!(cfg_org.streams.len(), 1);

        let filters = &cfg_org.streams.get("echo").unwrap().filters;
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
        )
        .unwrap();
        let cfg_oth = parse_config_json(
            r#"{
                "streams": {
                    "echo": {
                        "cmd": ["true"]
                    }
                }
            }"#,
        )
        .unwrap();

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
        )
        .unwrap();
        let cfg_oth = parse_config_json(
            r#"{
                "streams": {
                    "echo": {
                        "cmd": ["echo"]
                    }
                }
            }"#,
        )
        .unwrap();

        cfg_org.merge(cfg_oth).unwrap();
        cfg_org.setup().unwrap();
        assert!(cfg_org.streams.contains_key("echo"));
        assert_eq!(cfg_org.streams.len(), 1);
        let stream = cfg_org.streams.get("echo").unwrap();
        assert_eq!(stream.cmd.len(), 1);
        assert_eq!(stream.filters.len(), 1);
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
        )
        .unwrap();
        let cfg_oth = parse_config_json(
            r#"{
                "streams": {
                    "echo": {
                        "cmd": ["echo"],
                        {{FILTERS}}
                    }
                }
            }"#,
        )
        .unwrap();

        let res = cfg_org.merge(cfg_oth);
        assert!(res.is_err());
    }

    #[test]
    fn merge_config_same_concurrency() {
        let mut cfg_org = parse_config_json(
            r#"{
                "concurrency": 97,
                {{STREAMS}}
            }"#,
        )
        .unwrap();
        let cfg_oth = parse_config_json(
            r#"{
                "concurrency": 97
            }"#,
        )
        .unwrap();

        let res = cfg_org.merge(cfg_oth);
        assert!(res.is_ok());
        assert_eq!(cfg_org.concurrency, 97);
    }

    #[test]
    fn merge_config_distinct_concurrency() {
        let mut cfg_org = parse_config_json(
            r#"{
                "concurrency": 97,
                {{STREAMS}}
            }"#,
        )
        .unwrap();
        let cfg_oth = parse_config_json(
            r#"{
                "concurrency": 96
            }"#,
        )
        .unwrap();

        let res = cfg_org.merge(cfg_oth);
        assert!(res.is_err());
    }

    #[test]
    fn merge_config_one_concurrency() {
        let mut cfg_org = parse_config_json(
            r#"{
                "concurrency": 97,
                {{STREAMS}}
            }"#,
        )
        .unwrap();
        let cfg_oth = parse_config_json(r#"{}"#).unwrap();

        let res = cfg_org.merge(cfg_oth);
        assert!(res.is_ok());
        assert_eq!(cfg_org.concurrency, 97);
    }

    #[test]
    fn merge_config_right_concurrency() {
        let mut cfg_org = parse_config_json(
            r#"{
                {{STREAMS}}
            }"#,
        )
        .unwrap();
        let cfg_oth = parse_config_json(
            r#"{
                "concurrency": 97
            }"#,
        )
        .unwrap();

        let res = cfg_org.merge(cfg_oth);
        assert!(res.is_ok());
        assert_eq!(cfg_org.concurrency, 97);
    }

    #[test]
    fn merge_config_no_concurrency() {
        let mut cfg_org = parse_config_json(
            r#"{
                {{STREAMS}}
            }"#,
        )
        .unwrap();
        let cfg_oth = parse_config_json(r#"{ }"#).unwrap();

        let res = cfg_org.merge(cfg_oth);
        assert!(res.is_ok());
        assert_eq!(cfg_org.concurrency, num_cpus::get());
    }

    #[test]
    fn parse_config_no_state_rotation_interval() {
        let mut cfg_org = parse_config_json(
            r#"{
                {{PATTERNS}},
                {{STREAMS}}
            }"#,
        )
        .unwrap();
        assert!(cfg_org.setup().is_ok());
        assert_eq!(
            cfg_org.state_rotation_interval,
            Some(Duration::from_hours(24))
        );
    }

    #[test]
    fn parse_config_set_state_rotation_interval() {
        let mut cfg_org = parse_config_json(
            r#"{
                "state_rotation_interval": "6h",
                {{PATTERNS}},
                {{STREAMS}}
            }"#,
        )
        .unwrap();
        assert!(cfg_org.setup().is_ok());
        assert_eq!(
            cfg_org.state_rotation_interval,
            Some(Duration::from_hours(6))
        );
    }

    #[test]
    fn parse_config_null_state_rotation_interval() {
        let mut cfg_org = parse_config_json(
            r#"{
                "state_rotation_interval": null,
                {{PATTERNS}},
                {{STREAMS}}
            }"#,
        )
        .unwrap();
        assert!(cfg_org.setup().is_ok());
        assert_eq!(cfg_org.state_rotation_interval, None);
    }

    #[test]
    fn parse_config_no_log_level() {
        let mut cfg_org = parse_config_json(
            r#"{
                {{PATTERNS}},
                {{STREAMS}}
            }"#,
        )
        .unwrap();
        assert!(cfg_org.setup().is_ok());
        assert_eq!(cfg_org.log_level, LogLevel(Level::INFO));
    }

    #[test]
    fn parse_config_set_log_level() {
        let mut cfg_org = parse_config_json(
            r#"{
                "log_level": "WARN",
                {{PATTERNS}},
                {{STREAMS}}
            }"#,
        )
        .unwrap();
        assert!(cfg_org.setup().is_ok());
        assert_eq!(cfg_org.log_level, LogLevel(Level::WARN));
    }

    #[test]
    fn parse_config_trace_log_level() {
        assert!(
            parse_config_json(
                r#"{
                "log_level": "TRACE",
                {{PATTERNS}},
                {{STREAMS}}
            }"#,
            )
            .is_err(),
        );
    }

    #[test]
    fn merge_config_right_log_level() {
        let mut cfg_org = parse_config_json(
            r#"{
                {{STREAMS}}
            }"#,
        )
        .unwrap();
        let cfg_oth = parse_config_json(
            r#"{
                "log_level": "ERROR"
            }"#,
        )
        .unwrap();

        let res = cfg_org.merge(cfg_oth);
        assert!(res.is_ok());
        assert_eq!(cfg_org.log_level, LogLevel(Level::ERROR));
    }
}
