use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    sync::Arc,
};

use chrono::TimeDelta;
use regex::Regex;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::info;

use super::{
    messages::{Match, Time, MAT},
    Action, ActionFilter, LogEntry, Pattern, Patterns,
};
use crate::utils::parse_duration;

// Only names are serialized
// Only computed fields are not deserialized
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    actions: BTreeMap<String, Action>,
    #[serde(skip)]
    longuest_action_duration: TimeDelta,

    regex: Vec<String>,
    #[serde(skip)]
    compiled_regex: Vec<Regex>,
    // We want patterns to be ordered
    // This is necessary when using matches which contain multiple patterns
    #[serde(skip)]
    patterns: Arc<BTreeSet<Arc<Pattern>>>,

    retry: Option<u32>,
    #[serde(rename = "retryperiod")]
    retry_period: Option<String>,
    #[serde(skip)]
    retry_duration: Option<TimeDelta>,

    #[serde(skip)]
    name: String,
    #[serde(skip)]
    stream_name: String,
}

impl ActionFilter for Filter {
    fn full_name(&self) -> (&str, &str, &str) {
        (self.stream_name.as_ref(), self.name.as_ref(), "")
    }

    fn patterns(&self) -> &BTreeSet<Arc<Pattern>> {
        &self.patterns
    }
}

impl Filter {
    #[cfg(test)]
    pub fn from_name(stream_name: &str, filter_name: &str) -> Filter {
        Filter {
            stream_name: stream_name.into(),
            name: filter_name.into(),
            ..Filter::default()
        }
    }

    pub fn retry(&self) -> Option<u32> {
        self.retry
    }

    pub fn retry_duration(&self) -> Option<TimeDelta> {
        self.retry_duration
    }

    pub fn actions(&self) -> &BTreeMap<String, Action> {
        &self.actions
    }

    pub fn setup(
        &mut self,
        stream_name: &str,
        name: &str,
        config_patterns: &Patterns,
    ) -> Result<(), String> {
        self._setup(stream_name, name, config_patterns)
            .map_err(|msg| format!("filter {}: {}", name, msg))
    }

    fn _setup(
        &mut self,
        stream_name: &str,
        name: &str,
        config_patterns: &Patterns,
    ) -> Result<(), String> {
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

        let mut new_patterns = BTreeSet::new();
        let mut first = true;
        for regex in &self.regex {
            let mut regex_buf = regex.clone();
            for pattern in config_patterns.values() {
                if let Some(index) = regex.find(pattern.name_with_braces()) {
                    // we already `find` it, so we must be able to `rfind` it
                    #[allow(clippy::unwrap_used)]
                    if regex.rfind(pattern.name_with_braces()).unwrap() != index {
                        return Err(format!(
                            "pattern {} present multiple times in regex",
                            pattern.name_with_braces()
                        ));
                    }
                    if first {
                        new_patterns.insert(pattern.clone());
                    } else if !new_patterns.contains(pattern) {
                        return Err(format!(
                            "pattern {} is not present in the first regex but is present in a following regex. all regexes should contain the same set of regexes",
                            &pattern.name_with_braces()
                        ));
                    }
                } else if !first && new_patterns.contains(pattern) {
                    return Err(format!(
                            "pattern {} is present in the first regex but is not present in a following regex. all regexes should contain the same set of regexes",
                            &pattern.name_with_braces()
                        ));
                }
                regex_buf = regex_buf.replacen(pattern.name_with_braces(), &pattern.regex, 1);
            }
            let compiled = Regex::new(&regex_buf).map_err(|err| err.to_string())?;
            self.compiled_regex.push(compiled);
            first = false;
        }
        self.regex.clear();
        self.patterns = Arc::new(new_patterns);

        if self.actions.is_empty() {
            return Err("no actions configured".into());
        }

        for (key, action) in &mut self.actions {
            action.setup(stream_name, name, key, self.patterns.clone())?;
        }

        self.longuest_action_duration =
            self.actions.values().fold(TimeDelta::seconds(0), |acc, v| {
                v.after_duration()
                    .map_or(acc, |v| if v > acc { v } else { acc })
            });

        Ok(())
    }

    pub fn get_match(&self, line: &str) -> Option<Match> {
        for regex in &self.compiled_regex {
            if let Some(matches) = regex.captures(line) {
                if !self.patterns.is_empty() {
                    let mut result = Match::new();
                    for pattern in self.patterns.as_ref() {
                        // if the pattern is in an optional part of the regex, there may be no
                        // captured group for it.
                        if let Some(match_) = matches.name(pattern.name()) {
                            if pattern.not_an_ignore(match_.as_str()) {
                                result.push(match_.as_str().to_string());
                            }
                        }
                    }
                    if result.len() == self.patterns.len() {
                        info!("{}: match {:?}", self, result);
                        return Some(result);
                    }
                } else {
                    info!("{}: match []", self);
                    return Some(vec![".".to_string()]);
                }
            }
        }
        None
    }

    pub async fn send_actions(&'static self, m: &Match, t: Time, tx: &mpsc::Sender<MAT>) {
        for action in self.actions.values() {
            let mat = MAT {
                m: m.clone(),
                o: action,
                t: t + action.after_duration().unwrap_or_default(),
            };
            #[allow(clippy::unwrap_used)] // propagating panics is ok
            tx.send(mat).await.unwrap();
        }
    }

    pub fn is_outdated(entry: &LogEntry, now: &Time) -> bool {
        if entry.exec {
            entry.t + entry.f.longuest_action_duration < *now
        } else {
            entry.t + entry.f.retry_duration.unwrap_or_default() < *now
        }
    }
}

impl Display for Filter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.stream_name, self.name)
    }
}

impl PartialEq for Filter {
    fn eq(&self, other: &Self) -> bool {
        self.stream_name == other.stream_name && self.name == other.name
    }
}
impl Eq for Filter {}
impl Ord for Filter {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.stream_name.cmp(&other.stream_name) {
            Ordering::Equal => self.name.cmp(&other.name),
            o => o,
        }
    }
}
impl PartialOrd for Filter {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
pub mod tests {
    use crate::concepts::action::tests::{ok_action, ok_action_with_after};
    use crate::concepts::pattern::tests::{
        boubou_pattern_with_ignore, default_pattern, ok_pattern_with_ignore,
    };

    use super::*;

    pub fn ok_filter() -> Filter {
        let mut filter = Filter::default();
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
        let minute = TimeDelta::seconds(60);
        let two_minutes = TimeDelta::seconds(60 * 2);
        let two_minutes_str = "2m".to_string();

        // duration 0
        filter = ok_filter();
        filter.setup(&name, &name, &empty_patterns).unwrap();
        assert_eq!(filter.longuest_action_duration, TimeDelta::default());

        let minute_action = ok_action_with_after(minute_str.clone(), &minute_str);

        // duration 60
        filter = ok_filter();
        filter
            .actions
            .insert(minute_str.clone(), minute_action.clone());
        filter.setup(&name, &name, &empty_patterns).unwrap();
        assert_eq!(filter.longuest_action_duration, minute);

        let two_minutes_action = ok_action_with_after(two_minutes_str.clone(), &two_minutes_str);

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
        patterns.insert(name.clone(), pattern.clone().into());

        let unused_name = "unused".to_string();
        let mut unused_pattern = default_pattern();
        unused_pattern.regex = "compile[error".to_string();
        assert!(unused_pattern.setup(&unused_name).is_err());
        patterns.insert(unused_name.clone(), unused_pattern.clone().into());

        let boubou_name = "boubou".to_string();
        let mut boubou = boubou_pattern_with_ignore();
        boubou.setup(&boubou_name).unwrap();
        patterns.insert(boubou_name.clone(), boubou.clone().into());

        // correct regex replacement
        filter = Filter::default();
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
        filter = Filter::default();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("there <name> are two <name> s!".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_err());

        // two patterns in one regex
        filter = Filter::default();
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
        filter = Filter::default();
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
        filter = Filter::default();
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
        filter = Filter::default();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex.push("insert <name> here".to_string());
        filter.regex.push("also add <boubou> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_err());

        // multiple regexes with different patterns 2
        filter = Filter::default();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        filter.regex.push("also add <boubou> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_err());

        // multiple regexes with different patterns 3
        filter = Filter::default();
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
        patterns.insert(name.clone(), pattern.clone().into());

        let boubou_name = "boubou".to_string();
        let mut boubou = boubou_pattern_with_ignore();
        boubou.setup(&boubou_name).unwrap();
        patterns.insert(boubou_name.clone(), boubou.clone().into());

        // one simple regex
        filter = Filter::default();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex.push("insert <name> here$".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
        assert_eq!(filter.get_match("insert b here"), Some(vec!("b".into())));
        assert_eq!(filter.get_match("insert a here"), None);
        assert_eq!(filter.get_match("youpi b youpi"), None);
        assert_eq!(filter.get_match("insert  here"), None);

        // two patterns in one regex
        filter = Filter::default();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
        assert_eq!(
            filter.get_match("insert b here and bouboubou there"),
            Some(vec!("bouboubou".into(), "b".into()))
        );
        assert_eq!(filter.get_match("insert a here and bouboubou there"), None);
        assert_eq!(filter.get_match("insert b here and boubou there"), None);

        // multiple regexes with same pattern
        filter = Filter::default();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex.push("insert <name> here".to_string());
        filter.regex.push("also add <name> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
        assert_eq!(filter.get_match("insert a here"), None);
        assert_eq!(filter.get_match("insert b here"), Some(vec!("b".into())));
        assert_eq!(filter.get_match("also add a there"), None);
        assert_eq!(filter.get_match("also add b there"), Some(vec!("b".into())));

        // multiple regexes with same patterns
        filter = Filter::default();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        filter
            .regex
            .push("also add <boubou> here and <name> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
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
