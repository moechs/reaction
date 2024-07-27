use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    sync::mpsc::Sender,
    time::{Duration, SystemTime},
};

use log::info;
use regex::Regex;
use serde::Deserialize;

use crate::{
    action::Action,
    config::Patterns,
    messages::{Match, MAT},
    parse_duration::parse_duration,
    pattern::Pattern,
};

#[derive(Clone, Debug, Deserialize)]
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
    pub fn name(&self) -> FilterName {
        FilterName {
            stream: self.stream_name.clone(),
            filter: self.name.clone(),
        }
    }

    pub fn retry(&self) -> Option<u32> {
        self.retry
    }

    pub fn retry_duration(&self) -> Option<Duration> {
        self.retry_duration
    }

    pub fn setup(
        &mut self,
        stream_name: &str,
        name: &str,
        patterns: &Patterns,
    ) -> Result<(), String> {
        self._setup(stream_name, name, patterns)
            .map_err(|msg| format!("filter {}: {}", name, msg))
    }

    fn _setup(&mut self, stream_name: &str, name: &str, patterns: &Patterns) -> Result<(), String> {
        self.stream_name = stream_name.to_string();
        self.name = name.to_string();

        if self.name.is_empty() {
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
            self.retry_duration = Some(
                parse_duration(retry_period)
                    .map_err(|err| format!("failed to parse retry time: {}", err))?,
            );
            self.retry_period = None;
        }

        if self.regex.is_empty() {
            return Err("no regex configured".into());
        }

        let mut first = true;
        for regex in &self.regex {
            let mut regex_buf = regex.clone();
            for pattern in patterns.values() {
                if let Some(index) = regex.find(&pattern.name_with_braces) {
                    if regex.rfind(&pattern.name_with_braces).unwrap() != index {
                        return Err(format!(
                            "pattern {} present multiple times in regex",
                            &pattern.name_with_braces
                        ));
                    }
                    if first {
                        self.patterns.insert(pattern.clone());
                    } else if !self.patterns.contains(pattern) {
                        return Err(format!(
                            "pattern {} is not present in the first regex but is present in a following regex. all regexes should contain the same set of regexes",
                            &pattern.name_with_braces
                        ));
                    }
                } else if !first && self.patterns.contains(pattern) {
                    return Err(format!(
                            "pattern {} is present in the first regex but is not present in a following regex. all regexes should contain the same set of regexes",
                            &pattern.name_with_braces
                        ));
                }
                regex_buf = regex_buf.replacen(&pattern.name_with_braces, &pattern.regex, 1);
            }
            let compiled = Regex::new(&regex_buf).map_err(|err| err.to_string())?;
            self.compiled_regex.push(compiled);
            first = false;
        }
        self.regex.clear();

        if self.actions.is_empty() {
            return Err("no actions configured".into());
        }

        for (key, value) in &mut self.actions {
            value.setup(stream_name, name, key)?;
        }

        self.longuest_action_duration =
            self.actions
                .values()
                .fold(Duration::from_secs(0), |acc, v| {
                    v.after_duration
                        .map_or(acc, |v| if v > acc { v } else { acc })
                });

        Ok(())
    }

    pub fn get_match(&self, line: &str) -> Option<Match> {
        for regex in &self.compiled_regex {
            if let Some(matches) = regex.captures(line) {
                if !self.patterns.is_empty() {
                    let mut result = Match::new();
                    for pattern in &self.patterns {
                        let match_ = matches.name(&pattern.name).unwrap();
                        if pattern.not_an_ignore(match_.as_str()) {
                            result.push(match_.as_str().to_string());
                        }
                    }
                    if result.len() == self.patterns.len() {
                        info!("{}: match {:?}", self.name(), result);
                        return Some(result);
                    }
                } else {
                    info!("{}: match []", self.name());
                    return Some(vec![".".to_string()]);
                }
            }
        }
        None
    }

    pub fn send_actions(&self, m: &Match, t: SystemTime, tx: &Sender<MAT>) {
        for action in self.actions.values() {
            tx.send(MAT {
                m: m.clone(),
                a: action.name(),
                t,
            })
            .unwrap();
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FilterName {
    pub stream: String,
    pub filter: String,
}
impl Display for FilterName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.stream, self.filter)
    }
}

#[cfg(test)]
pub mod tests {

    use crate::action::tests::ok_action;
    use crate::pattern::tests::{
        boubou_pattern_with_ignore, default_pattern, ok_pattern_with_ignore,
    };

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
    fn setup_missing_config() {
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
    fn setup_retry() {
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
    fn setup_longuest_action_duration() {
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
    fn setup_regexes() {
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
        let mut boubou = boubou_pattern_with_ignore();
        boubou.setup(&boubou_name).unwrap();
        patterns.insert(boubou_name.clone(), boubou.clone());

        // correct regex replacement
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

        // same pattern two times in regex
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("there <name> are two <name> s!".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_err());

        // two patterns in one regex
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
        assert_eq!(
            filter.compiled_regex[0].to_string(),
            Regex::new("insert (?P<name>[abc]) here and (?P<boubou>(?:bou){1,3}) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(filter.patterns.len(), 2);
        let stored_pattern = filter.patterns.first().unwrap();
        assert_eq!(stored_pattern.regex, boubou.regex);
        let stored_pattern = filter.patterns.last().unwrap();
        assert_eq!(stored_pattern.regex, pattern.regex);

        // multiple regexes with same pattern
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

        // multiple regexes with same patterns
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
            Regex::new("insert (?P<name>[abc]) here and (?P<boubou>(?:bou){1,3}) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(
            filter.compiled_regex[1].to_string(),
            Regex::new("also add (?P<boubou>(?:bou){1,3}) here and (?P<name>[abc]) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(filter.patterns.len(), 2);
        let stored_pattern = filter.patterns.first().unwrap();
        assert_eq!(stored_pattern.regex, boubou.regex);
        let stored_pattern = filter.patterns.last().unwrap();
        assert_eq!(stored_pattern.regex, pattern.regex);

        // multiple regexes with different patterns 1
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex.push("insert <name> here".to_string());
        filter.regex.push("also add <boubou> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_err());

        // multiple regexes with different patterns 2
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        filter.regex.push("also add <boubou> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_err());

        // multiple regexes with different patterns 3
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex.push("also add <boubou> there".to_string());
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_err());
    }

    #[test]
    fn get_match() {
        let name = "name".to_string();
        let mut filter;

        // make a Patterns
        let mut patterns = Patterns::new();

        let mut pattern = ok_pattern_with_ignore();
        pattern.setup(&name).unwrap();
        patterns.insert(name.clone(), pattern.clone());

        let boubou_name = "boubou".to_string();
        let mut boubou = boubou_pattern_with_ignore();
        boubou.setup(&boubou_name).unwrap();
        patterns.insert(boubou_name.clone(), boubou.clone());

        // one simple regex
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex.push("insert <name> here$".to_string());
        filter.setup(&name, &name, &patterns).unwrap();
        assert_eq!(filter.get_match("insert b here"), Some(vec!("b".into())));
        assert_eq!(filter.get_match("insert a here"), None);
        assert_eq!(filter.get_match("youpi b youpi"), None);
        assert_eq!(filter.get_match("insert  here"), None);

        // two patterns in one regex
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        filter.setup(&name, &name, &patterns).unwrap();
        assert_eq!(
            filter.get_match("insert b here and bouboubou there"),
            Some(vec!("bouboubou".into(), "b".into()))
        );
        assert_eq!(filter.get_match("insert a here and bouboubou there"), None);
        assert_eq!(filter.get_match("insert b here and boubou there"), None);

        // multiple regexes with same pattern
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex.push("insert <name> here".to_string());
        filter.regex.push("also add <name> there".to_string());
        filter.setup(&name, &name, &patterns).unwrap();
        assert_eq!(filter.get_match("insert a here"), None);
        assert_eq!(filter.get_match("insert b here"), Some(vec!("b".into())));
        assert_eq!(filter.get_match("also add a there"), None);
        assert_eq!(filter.get_match("also add b there"), Some(vec!("b".into())));

        // multiple regexes with same patterns
        filter = default_filter();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        filter
            .regex
            .push("also add <boubou> here and <name> there".to_string());
        filter.setup(&name, &name, &patterns).unwrap();
        assert_eq!(
            filter.get_match("insert b here and bouboubou there"),
            Some(vec!("bouboubou".into(), "b".into()))
        );
        assert_eq!(
            filter.get_match("also add bouboubou here and b there"),
            Some(vec!("bouboubou".into(), "b".into()))
        );
        assert_eq!(filter.get_match("insert a here and bouboubou there"), None);
        assert_eq!(
            filter.get_match("also add bouboubou here and a there"),
            None
        );
        assert_eq!(filter.get_match("insert b here and boubou there"), None);
        assert_eq!(filter.get_match("also add boubou here and b there"), None);
    }
}
