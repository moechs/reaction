use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use regex::Regex;
use serde::Deserialize;

use crate::{action::Action, config::Patterns, parse_duration::parse_duration, pattern::Pattern};

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

        if self.name.len() == 0 {
            return Err("filter name is empty".into());
        }
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
                if let Some(index) = regex.find(&pattern.name_with_braces) {
                    if regex.rfind(&pattern.name_with_braces).unwrap() != index {
                        return Err(format!(
                            "pattern {} present multiple times in regex",
                            &pattern.name_with_braces
                        ));
                    }
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

#[cfg(test)]
pub mod tests {

    use crate::action::tests::ok_action;
    use crate::pattern::tests::default_pattern;

    use super::*;

    pub fn default_filter() -> Filter {
        Filter {
            name: "".into(),
            stream_name: "".into(),
            actions: BTreeMap::new(),
            patterns: BTreeSet::new(),
            regex: Vec::new(),
            compiled_regex: Vec::new(),
            retry: None,
            retry_period: None,
            retry_duration: None,
            longuest_action_duration: Duration::default(),
        }
    }

    pub fn ok_filter() -> Filter {
        let mut filter = default_filter();
        let name = "name".to_string();
        filter.regex = vec!["reg".into()];
        filter.actions.insert(name.clone(), ok_action());
        filter
    }

    #[test]
    fn missing_config() {
        let mut filter;
        let name = "name".to_string();
        let empty_patterns = Patterns::new();

        // action but no regex
        filter = ok_filter();
        filter.regex = Vec::new();
        assert!(filter.setup(&name, &name, &empty_patterns).is_err());

        // regex but no action
        filter = ok_filter();
        filter.actions = BTreeMap::new();
        assert!(filter.setup(&name, &name, &empty_patterns).is_err());

        // ok
        filter = ok_filter();
        assert!(filter.setup(&name, &name, &empty_patterns).is_ok());
    }

    #[test]
    fn retry() {
        let mut filter;
        let name = "name".to_string();
        let empty_patterns = Patterns::new();

        // retry but no retry_period
        filter = ok_filter();
        filter.retry = Some(2);
        assert!(filter.setup(&name, &name, &empty_patterns).is_err());

        // retry_period but no retry
        filter = ok_filter();
        filter.retry_period = Some("2d".into());
        assert!(filter.setup(&name, &name, &empty_patterns).is_err());

        // invalid retry_period
        filter = ok_filter();
        filter.retry = Some(2);
        filter.retry_period = Some("2".into());
        assert!(filter.setup(&name, &name, &empty_patterns).is_err());

        // ok
        filter = ok_filter();
        filter.retry = Some(2);
        filter.retry_period = Some("2d".into());
        assert!(filter.setup(&name, &name, &empty_patterns).is_ok());
    }

    #[test]
    fn longuest_action_duration() {
        let mut filter;
        let name = "name".to_string();
        let empty_patterns = Patterns::new();
        let minute_str = "1m".to_string();
        let minute = Duration::from_secs(60);
        let two_minutes = Duration::from_secs(60 * 2);
        let two_minutes_str = "2m".to_string();

        // duration 0
        filter = ok_filter();
        filter.setup(&name, &name, &empty_patterns).unwrap();
        assert_eq!(filter.longuest_action_duration, Duration::default());

        let mut minute_action = ok_action();
        minute_action.after_duration = Some(minute);

        // duration 60
        filter = ok_filter();
        filter.actions.insert(minute_str.clone(), minute_action);
        filter.setup(&name, &name, &empty_patterns).unwrap();
        assert_eq!(filter.longuest_action_duration, minute);

        let mut minute_action = ok_action();
        minute_action.after_duration = Some(minute);

        let mut two_minutes_action = ok_action();
        two_minutes_action.after_duration = Some(two_minutes);

        // duration 120
        filter = ok_filter();
        filter.actions.insert(two_minutes_str, two_minutes_action);
        filter.actions.insert(minute_str, minute_action);
        filter.setup(&name, &name, &empty_patterns).unwrap();
        assert_eq!(filter.longuest_action_duration, two_minutes);
    }

    #[test]
    fn regexes() {
        let name = "name".to_string();
        let mut filter;

        // make a Patterns
        let mut patterns = Patterns::new();

        let mut pattern = default_pattern();
        pattern.regex = "[abc]".to_string();
        assert!(pattern.setup(&name).is_ok());
        patterns.insert(name.clone(), pattern.clone());

        let unused_name = "unused".to_string();
        let mut unused_pattern = default_pattern();
        unused_pattern.regex = "compile[error".to_string();
        assert!(unused_pattern.setup(&unused_name).is_err());
        patterns.insert(unused_name.clone(), unused_pattern.clone());

        let boubou_name = "boubou".to_string();
        let mut boubou = default_pattern();
        boubou.regex = "(?:bou){2}".to_string();
        assert!(boubou.setup(&boubou_name).is_ok());
        patterns.insert(boubou_name.clone(), boubou.clone());

        // TODO correct regex replacement
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex.push("insert <name> here$".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
        assert_eq!(
            filter.compiled_regex[0].to_string(),
            Regex::new("insert (?P<name>[abc]) here$")
                .unwrap()
                .to_string()
        );
        assert_eq!(filter.patterns.len(), 1);
        let stored_pattern = filter.patterns.first().unwrap();
        assert_eq!(stored_pattern.regex, pattern.regex);

        // TODO same pattern two times in regex
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("there <name> are two <name> s!".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_err());

        // TODO two patterns in one regex
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
        assert_eq!(
            filter.compiled_regex[0].to_string(),
            Regex::new("insert (?P<name>[abc]) here and (?P<boubou>(?:bou){2}) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(filter.patterns.len(), 2);
        let stored_pattern = filter.patterns.first().unwrap();
        assert_eq!(stored_pattern.regex, boubou.regex);
        let stored_pattern = filter.patterns.last().unwrap();
        assert_eq!(stored_pattern.regex, pattern.regex);

        // TODO multiple regexes with same pattern
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex.push("insert <name> here".to_string());
        filter.regex.push("also add <name> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
        assert_eq!(
            filter.compiled_regex[0].to_string(),
            Regex::new("insert (?P<name>[abc]) here")
                .unwrap()
                .to_string()
        );
        assert_eq!(
            filter.compiled_regex[1].to_string(),
            Regex::new("also add (?P<name>[abc]) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(filter.patterns.len(), 1);
        let stored_pattern = filter.patterns.first().unwrap();
        assert_eq!(stored_pattern.regex, pattern.regex);

        // TODO multiple regexes with same patterns
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        filter
            .regex
            .push("also add <boubou> here and <name> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
        assert_eq!(
            filter.compiled_regex[0].to_string(),
            Regex::new("insert (?P<name>[abc]) here and (?P<boubou>(?:bou){2}) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(
            filter.compiled_regex[1].to_string(),
            Regex::new("also add (?P<boubou>(?:bou){2}) here and (?P<name>[abc]) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(filter.patterns.len(), 2);
        let stored_pattern = filter.patterns.first().unwrap();
        assert_eq!(stored_pattern.regex, boubou.regex);
        let stored_pattern = filter.patterns.last().unwrap();
        assert_eq!(stored_pattern.regex, pattern.regex);

        // TODO multiple regexes with different patterns
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex.push("insert <name> here".to_string());
        filter.regex.push("also add <boubou> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
        assert_eq!(
            filter.compiled_regex[0].to_string(),
            Regex::new("insert (?P<name>[abc]) here")
                .unwrap()
                .to_string()
        );
        assert_eq!(
            filter.compiled_regex[1].to_string(),
            Regex::new("also add (?P<boubou>(?:bou){2}) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(filter.patterns.len(), 2);
        let stored_pattern = filter.patterns.first().unwrap();
        assert_eq!(stored_pattern.regex, boubou.regex);
        let stored_pattern = filter.patterns.last().unwrap();
        assert_eq!(stored_pattern.regex, pattern.regex);
    }
}
