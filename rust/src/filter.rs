use std::{collections::{BTreeMap, BTreeSet}, time::Duration};

use regex::Regex;
use serde::Deserialize;

use crate::{action::Action, pattern::Pattern, config::Patterns, parse_duration::parse_duration};

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


impl Filter {
    pub fn setup(
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
