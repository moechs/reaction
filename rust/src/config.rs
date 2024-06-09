#![allow(dead_code)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde::Deserialize;

use crate::parse_duration::parse_duration;

pub type Patterns = BTreeMap<String, Pattern>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    concurrency: usize,
    patterns: Patterns,
    streams: BTreeMap<String, Stream>,
    #[serde(default)]
    start: Vec<Vec<String>>,
    #[serde(default)]
    stop: Vec<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    regex: String,
    #[serde(default)]
    ignore: Vec<String>,

    #[serde(default, rename = "ignoreregex")]
    ignore_regex: Vec<String>,
    #[serde(skip)]
    compiled_ignore_regex: Vec<Regex>,

    #[serde(skip)]
    name: String,
    #[serde(skip)]
    name_with_braces: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    cmd: Vec<String>,
    filters: BTreeMap<String, Filter>,

    #[serde(skip)]
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    actions: BTreeMap<String, Action>,
    #[serde(skip)]
    longuest_action_duration: Duration,

    regex: Vec<String>,
    #[serde(skip)]
    compiled_regex: Vec<Regex>,
    // We want patterns to be ordered
    // This is necessary when using matches which contain multiple patterns
    #[serde(skip)]
    patterns: BTreeSet<Pattern>,

    retry: Option<u32>,
    #[serde(rename = "retryperiod")]
    retry_period: Option<String>,
    retry_duration: Option<Duration>,

    #[serde(skip)]
    name: String,
    #[serde(skip)]
    stream_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    cmd: Vec<String>,

    // TODO one shot time deserialization
    after: Option<String>,
    #[serde(skip)]
    after_duration: Option<Duration>,

    #[serde(rename = "onexit", default = "set_false")]
    on_exit: bool,

    #[serde(skip)]
    name: String,
    #[serde(skip)]
    filter_name: String,
    #[serde(skip)]
    stream_name: String,
}

fn set_0() -> u32 {
    0
}

fn set_false() -> bool {
    false
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

impl Pattern {
    pub fn setup(&mut self, name: &String) -> Result<(), String> {
        self._setup(name)
            .or_else(|msg| Err(format!("pattern {}: {}", name, msg)))
    }

    pub fn _setup(&mut self, name: &String) -> Result<(), String> {
        self.name = name.clone();
        self.name_with_braces = format!("<{}>", name);

        if self.regex.len() == 0 {
            return Err("regex is empty".into());
        }
        let compiled =
            Regex::new(&format!("^{}$", self.regex)).or_else(|err| Err(err.to_string()))?;

        self.regex = format!("(?P<{}>{})", self.name, self.regex);

        for ignore in &self.ignore {
            if !compiled.is_match(&ignore) {
                return Err(format!(
                    "ignore '{}' doesn't match pattern. It should be fixed or removed.",
                    ignore,
                ));
            }
        }

        for ignore_regex in &self.ignore_regex {
            let compiled_ignore = Regex::new(&format!("^{}$", ignore_regex)).or_else(|err| {
                Err(format!(
                    "ignoreregex '{}': {}",
                    ignore_regex,
                    err.to_string()
                ))
            })?;

            self.compiled_ignore_regex.push(compiled_ignore);
        }

        Ok(())
    }
}

// This is required to be added to a BTreeSet
// We compare Patterns by their names, which are unique.
// This is enforced by Patterns' names coming from their keys in a BTreeMap in Config
impl Ord for Pattern {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}
impl PartialOrd for Pattern {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Eq for Pattern {}
impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Stream {
    fn setup(&mut self, name: &String, patterns: &Patterns) -> Result<(), String> {
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

impl Filter {
    fn setup(
        &mut self,
        stream_name: &String,
        name: &String,
        patterns: &Patterns,
    ) -> Result<(), String> {
        self._setup(stream_name, name, patterns)
            .or_else(|msg| Err(format!("filter {}: {}", name, msg)))
    }

    fn _setup(
        &mut self,
        stream_name: &String,
        name: &String,
        patterns: &Patterns,
    ) -> Result<(), String> {
        self.stream_name = stream_name.clone();
        self.name = name.clone();

        if self.name.contains('.') {
            return Err("character '.' is not allowed in filter name".into());
        }

        if self.retry.is_some() != self.retry_period.is_some() {
            return Err("retry and retryperiod must be specified one with each other".into());
        }

        if self.retry.is_some_and(|r| r < 2) {
            return Err("retry has been specified but is < 2".into());
        }

        if let Some(retry_period) = &self.retry_period {
            self.retry_duration =
                Some(parse_duration(retry_period).or_else(|err| {
                    Err(format!("failed to parse retry time: {}", err.to_string()))
                })?);
        }

        if self.regex.len() == 0 {
            return Err("no regex configured".into());
        }

        for regex in &self.regex {
            let mut regex_buf = regex.clone();
            for (_pattern_name, pattern) in patterns {
                if regex.contains(&pattern.name_with_braces) {
                    self.patterns.insert(pattern.clone());
                }
                regex_buf = regex_buf.replacen(&pattern.name_with_braces, &pattern.regex, 1)
            }
            let compiled = Regex::new(&regex_buf).or_else(|err| Err(err.to_string()))?;
            self.compiled_regex.push(compiled);
        }

        if self.actions.len() == 0 {
            return Err("no actions configured".into());
        }

        for (key, value) in &mut self.actions {
            value.setup(&stream_name, &name, key)?;
        }

        self.longuest_action_duration = self
            .actions
            .iter()
            .map(|(_, v)| v.after_duration.unwrap_or(Duration::from_secs(0)))
            .max()
            .unwrap(); // safe because we already tested that actions.len() > 0

        Ok(())
    }
}

impl Action {
    fn setup(
        &mut self,
        stream_name: &String,
        filter_name: &String,
        name: &String,
    ) -> Result<(), String> {
        self._setup(stream_name, filter_name, name)
            .or_else(|msg| Err(format!("action {}: {}", name, msg)))
    }
    fn _setup(
        &mut self,
        stream_name: &String,
        filter_name: &String,
        name: &String,
    ) -> Result<(), String> {
        self.stream_name = stream_name.clone();
        self.filter_name = filter_name.clone();
        self.name = name.clone();

        if self.name.contains('.') {
            return Err("character '.' is not allowed in filter name".into());
        }

        if let Some(after) = &self.after {
            self.after_duration =
                Some(parse_duration(after).or_else(|err| {
                    Err(format!("failed to parse after time: {}", err.to_string()))
                })?);
        } else if self.on_exit {
            return Err("cannot have `onexit: true`, without an `after` directive".into());
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
