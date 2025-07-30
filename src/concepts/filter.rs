use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    hash::Hash,
    sync::Arc,
};

use chrono::TimeDelta;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::parse_duration;
use super::{Action, Match, Pattern, Patterns};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum Duplicate {
    #[default]
    #[serde(rename = "extend")]
    Extend,
    #[serde(rename = "ignore")]
    Ignore,
    #[serde(rename = "rerun")]
    Rerun,
}

// Only names are serialized
// Only computed fields are not deserialized
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    #[serde(skip)]
    longuest_action_duration: TimeDelta,

    regex: Vec<String>,
    #[serde(skip)]
    compiled_regex: Vec<Regex>,
    // We want patterns to be ordered
    // This is necessary when using matches which contain multiple patterns
    #[serde(skip)]
    patterns: Arc<BTreeSet<Arc<Pattern>>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    retry: Option<u32>,
    #[serde(rename = "retryperiod", skip_serializing_if = "Option::is_none")]
    retry_period: Option<String>,
    #[serde(skip)]
    retry_duration: Option<TimeDelta>,

    #[serde(default)]
    duplicate: Duplicate,

    actions: BTreeMap<String, Action>,

    #[serde(skip)]
    name: String,
    #[serde(skip)]
    stream_name: String,
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

    #[cfg(test)]
    pub fn from_name_and_patterns(
        stream_name: &str,
        filter_name: &str,
        patterns: Vec<Pattern>,
    ) -> Filter {
        Filter {
            stream_name: stream_name.into(),
            name: filter_name.into(),
            patterns: Arc::new(patterns.into_iter().map(Arc::new).collect()),
            ..Filter::default()
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn stream_name(&self) -> &str {
        &self.stream_name
    }

    pub fn retry(&self) -> Option<u32> {
        self.retry
    }

    pub fn retry_duration(&self) -> Option<TimeDelta> {
        self.retry_duration
    }

    pub fn longuest_action_duration(&self) -> TimeDelta {
        self.longuest_action_duration
    }

    pub fn regex(&self) -> &Vec<String> {
        &self.regex
    }

    pub fn duplicate(&self) -> Duplicate {
        self.duplicate
    }

    pub fn actions(&self) -> &BTreeMap<String, Action> {
        &self.actions
    }

    pub fn patterns(&self) -> &BTreeSet<Arc<Pattern>> {
        &self.patterns
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
            return Err("retry must be >= 2. Remove 'retry' and 'retryperiod' to trigger at the first occurence.".into());
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
        let mut new_regex = Vec::new();
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
            new_regex.push(regex_buf.clone());
            let compiled = Regex::new(&regex_buf).map_err(|err| err.to_string())?;
            self.compiled_regex.push(compiled);
            first = false;
        }
        self.regex = new_regex;
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
                        // if the pattern is in an optional part of the regex,
                        // there may be no captured group for it.
                        if let Some(match_) = matches.name(pattern.name()) {
                            if !pattern.is_ignore(match_.as_str()) {
                                let mut match_ = match_.as_str().to_string();
                                pattern.normalize(&mut match_);
                                result.push(match_);
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

    /// Test that the patterns map conforms to the filter's patterns.
    /// Then returns a corresponding [`Match`].
    pub fn get_match_from_patterns(
        &self,
        mut patterns: BTreeMap<Arc<Pattern>, String>,
    ) -> Result<Match, String> {
        // Check pattern length
        if patterns.len() != self.patterns().len() {
            return Err(format!(
                "{} patterns specified, while the {}.{} filter has {} pattern: ({})",
                patterns.len(),
                self.stream_name(),
                self.name(),
                self.patterns().len(),
                self.patterns()
                    .iter()
                    .map(|pattern| pattern.name().clone())
                    .reduce(|acc, pattern| acc + ", " + &pattern)
                    .unwrap_or("".into()),
            ));
        }

        for (pattern, match_) in &mut patterns {
            if self.patterns.get(pattern).is_none() {
                return Err(format!(
                    "pattern {} is not present in the filter {}.{}",
                    pattern.name(),
                    self.stream_name,
                    self.name
                ));
            }

            if !pattern.is_match(match_) {
                return Err(format!(
                    "'{}' doesn't match pattern {}",
                    match_,
                    pattern.name(),
                ));
            }

            if pattern.is_ignore(match_) {
                return Err(format!(
                    "'{}' is explicitly ignored by pattern {}",
                    match_,
                    pattern.name(),
                ));
            }

            pattern.normalize(match_);
        }

        for pattern in self.patterns.iter() {
            if !patterns.contains_key(pattern) {
                return Err(format!(
                    "pattern {} is missing, because it's in the filter {}.{}",
                    pattern.name(),
                    self.stream_name,
                    self.name
                ));
            }
        }

        Ok(patterns.into_values().collect())
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
impl Hash for Filter {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.stream_name.hash(state);
        self.name.hash(state);
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
impl Filter {
    /// Test-only constructor designed to be easy to call
    pub fn new(
        actions: Vec<Action>,
        regex: Vec<&str>,
        retry: Option<u32>,
        retry_period: Option<&str>,
        stream_name: &str,
        name: &str,
        duplicate: Duplicate,
        config_patterns: &Patterns,
    ) -> Self {
        let mut filter = Self {
            actions: actions.into_iter().map(|a| (a.name().into(), a)).collect(),
            regex: regex.into_iter().map(|s| s.into()).collect(),
            retry,
            retry_period: retry_period.map(|s| s.into()),
            duplicate,
            ..Default::default()
        };
        filter.setup(stream_name, name, config_patterns).unwrap();
        filter
    }

    pub fn new_static(
        actions: Vec<Action>,
        regex: Vec<&str>,
        retry: Option<u32>,
        retry_period: Option<&str>,
        stream_name: &str,
        name: &str,
        duplicate: Duplicate,
        config_patterns: &Patterns,
    ) -> &'static Self {
        Box::leak(Box::new(Self::new(
            actions,
            regex,
            retry,
            retry_period,
            stream_name,
            name,
            duplicate,
            config_patterns,
        )))
    }
}

#[cfg(test)]
pub mod tests {
    use crate::concepts::action::tests::{ok_action, ok_action_with_after};
    use crate::concepts::pattern::tests::{
        boubou_pattern_with_ignore, default_pattern, number_pattern, ok_pattern_with_ignore,
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

        let mut pattern = ok_pattern_with_ignore();
        pattern.setup(&name).unwrap();
        let pattern = Arc::new(pattern);

        let boubou_name = "boubou".to_string();
        let mut boubou = boubou_pattern_with_ignore();
        boubou.setup(&boubou_name).unwrap();
        let boubou = Arc::new(boubou);

        let patterns = Patterns::from([
            (name.clone(), pattern.clone()),
            (boubou_name.clone(), boubou.clone()),
        ]);

        let number_name = "number".to_string();
        let mut number_pattern = number_pattern();
        number_pattern.setup(&number_name).unwrap();
        let number_pattern = Arc::new(number_pattern);

        // one simple regex
        filter = Filter::default();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex.push("insert <name> here$".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
        assert_eq!(filter.get_match("insert b here"), Some(vec!("b".into())));
        assert_eq!(filter.get_match("insert a here"), None);
        assert_eq!(filter.get_match("youpi b youpi"), None);
        assert_eq!(filter.get_match("insert  here"), None);

        // Ok
        assert_eq!(
            filter.get_match_from_patterns(BTreeMap::from([(pattern.clone(), "b".into())])),
            Ok(vec!("b".into()))
        );
        // Doesn't match
        assert!(filter
            .get_match_from_patterns(BTreeMap::from([(pattern.clone(), "abc".into())]))
            .is_err());
        // Ignored match
        assert!(filter
            .get_match_from_patterns(BTreeMap::from([(pattern.clone(), "a".into())]))
            .is_err());
        // Bad pattern
        assert!(filter
            .get_match_from_patterns(BTreeMap::from([(boubou.clone(), "bou".into())]))
            .is_err());
        // Bad number of patterns
        assert!(filter
            .get_match_from_patterns(BTreeMap::from([
                (pattern.clone(), "b".into()),
                (boubou.clone(), "bou".into()),
            ]))
            .is_err());
        // Bad number of patterns
        assert!(filter.get_match_from_patterns(BTreeMap::from([])).is_err());

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

        // Ok
        assert_eq!(
            filter.get_match_from_patterns(BTreeMap::from([
                (pattern.clone(), "b".into()),
                (boubou.clone(), "bou".into()),
            ])),
            // Reordered by pattern name
            Ok(vec!("bou".into(), "b".into()))
        );
        // Doesn't match
        assert!(filter
            .get_match_from_patterns(BTreeMap::from([
                (pattern.clone(), "abc".into()),
                (boubou.clone(), "bou".into()),
            ]))
            .is_err());
        // Ignored match
        assert!(filter
            .get_match_from_patterns(BTreeMap::from([
                (pattern.clone(), "b".into()),
                (boubou.clone(), "boubou".into()),
            ]))
            .is_err());
        // Bad pattern
        assert!(filter
            .get_match_from_patterns(BTreeMap::from([
                (pattern.clone(), "b".into()),
                (number_pattern.clone(), "1".into()),
            ]))
            .is_err());
        // Bad number of patterns
        assert!(filter
            .get_match_from_patterns(BTreeMap::from([
                (pattern.clone(), "b".into()),
                (boubou.clone(), "bou".into()),
                (number_pattern.clone(), "1".into()),
            ]))
            .is_err());
        // Bad number of patterns
        assert!(filter.get_match_from_patterns(BTreeMap::from([])).is_err());

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
