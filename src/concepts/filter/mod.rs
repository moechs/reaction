use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    hash::Hash,
    sync::Arc,
    time::Duration,
};

use reaction_plugin::time::parse_duration;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{Action, Match, Pattern, PatternType, Patterns};

use as_regex::FilterMethodRegex;
use as_struct::FilterMethodStruct;

mod as_regex;
mod as_struct;

// serde(tag = "type") doesn't support omitted type,
// so we must encode type and underlying options separately
#[derive(Default, Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub enum FilterType {
    #[default]
    #[serde(rename = "regex")]
    Regex,
    #[serde(rename = "struct")]
    Struct,
}

impl FilterType {
    pub fn is_default(&self) -> bool {
        *self == FilterType::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FilterMethod {
    #[serde(rename = "regex")]
    Regex(FilterMethodRegex),
    #[serde(rename = "struct")]
    Struct(FilterMethodStruct),
}

// Can't derive(Default) when the enum wraps a struct
// (even if the struct implements Default)
impl Default for FilterMethod {
    fn default() -> Self {
        Self::Regex(FilterMethodRegex::default())
    }
}

pub trait FilterMethodAs {
    fn setup(&mut self, config_patterns: &Patterns) -> Result<PatternSet, String>;
    fn get_match(&self, line: &str) -> Option<Match>;
    fn hint_regexes(&self) -> Vec<String>;
}

// We want patterns to be ordered
// This is necessary when using matches which contain multiple patterns
pub type PatternSet = Arc<BTreeSet<Arc<Pattern>>>;

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
    pub longuest_action_duration: Duration,
    #[serde(skip)]
    pub has_ip: bool,

    #[serde(
        default,
        rename = "type",
        skip_serializing_if = "FilterType::is_default"
    )]
    pub filter_type: FilterType,
    #[serde(skip)]
    pub filter_method: FilterMethod,
    #[serde(flatten)]
    pub option_regex: Option<FilterMethodRegex>,
    #[serde(flatten)]
    pub option_struct: Option<FilterMethodStruct>,

    #[serde(skip)]
    pub patterns: PatternSet,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<u32>,
    #[serde(rename = "retryperiod", skip_serializing_if = "Option::is_none")]
    pub retry_period: Option<String>,
    #[serde(skip)]
    pub retry_duration: Option<Duration>,

    #[serde(default)]
    pub duplicate: Duplicate,

    pub actions: BTreeMap<String, Action>,

    #[serde(skip)]
    pub name: String,
    #[serde(skip)]
    pub stream_name: String,
    // // Plugin-specific
    // #[serde(default, rename = "type")]
    // pub filter_type: Option<String>,
    // #[serde(default = "null_value")]
    // pub options: Value,
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
                    .map_err(|err| format!("failed to parse retry period: {}", err))?,
            );
            self.retry_period = None;
        }

        dbg!(&self);

        match &self.filter_type {
            FilterType::Regex => {
                if let Some(method_regex) = self.option_regex.take() {
                    self.filter_method = FilterMethod::Regex(method_regex);
                } else {
                    return Err("type set to 'regex' but no 'regex' given".into());
                }
                if self.option_struct.is_some() {
                    return Err("type set to regex but there is an 'options' attribute".into());
                }
            }
            FilterType::Struct => {
                if let Some(method_struct) = self.option_struct.take() {
                    self.filter_method = FilterMethod::Struct(method_struct);
                } else {
                    return Err("type set to 'struct' but no 'options' given".into());
                }
                if self.option_regex.is_some() {
                    return Err("type set to struct but there is a 'regex' attribute".into());
                }
            }
        }

        self.patterns = match &mut self.filter_method {
            FilterMethod::Regex(method_regex) => method_regex.setup(config_patterns),
            FilterMethod::Struct(method_struct) => method_struct.setup(config_patterns),
        }?;

        if self.actions.is_empty() {
            return Err("no actions configured".into());
        }

        for (key, action) in &mut self.actions {
            action.setup(stream_name, name, key, self.patterns.clone())?;
        }
        self.has_ip = self
            .actions
            .values()
            .any(|action| action.ipv4only || action.ipv6only);

        self.longuest_action_duration =
            self.actions
                .values()
                .fold(Duration::from_secs(0), |acc, v| {
                    v.after_duration
                        .map_or(acc, |v| if v > acc { v } else { acc })
                });

        Ok(())
    }

    pub fn hint_regexes(&self) -> Vec<String> {
        match &self.filter_method {
            FilterMethod::Regex(method_regex) => method_regex.hint_regexes(),
            FilterMethod::Struct(method_struct) => method_struct.hint_regexes(),
        }
    }

    pub fn get_match(&self, line: &str) -> Option<Match> {
        match &self.filter_method {
            FilterMethod::Regex(method_regex) => method_regex.get_match(line),
            FilterMethod::Struct(method_struct) => method_struct.get_match(line),
        }
    }

    /// Test that the patterns map conforms to the filter's patterns.
    /// Then returns a corresponding [`Match`].
    pub fn get_match_from_patterns(
        &self,
        mut patterns: BTreeMap<Arc<Pattern>, String>,
    ) -> Result<Match, String> {
        // Check pattern length
        if patterns.len() != self.patterns.len() {
            return Err(format!(
                "{} patterns specified, while the {}.{} filter has {} pattern: ({})",
                patterns.len(),
                self.stream_name,
                self.name,
                self.patterns.len(),
                self.patterns
                    .iter()
                    .map(|pattern| pattern.name.clone())
                    .reduce(|acc, pattern| acc + ", " + &pattern)
                    .unwrap_or("".into()),
            ));
        }

        for (pattern, match_) in &mut patterns {
            if self.patterns.get(pattern).is_none() {
                return Err(format!(
                    "pattern {} is not present in the filter {}.{}",
                    pattern.name, self.stream_name, self.name
                ));
            }

            if !pattern.is_match(match_) {
                return Err(format!(
                    "'{}' doesn't match pattern {}",
                    match_, pattern.name,
                ));
            }

            if pattern.is_ignore(match_) {
                return Err(format!(
                    "'{}' is explicitly ignored by pattern {}",
                    match_, pattern.name,
                ));
            }

            pattern.normalize(match_);
        }

        for pattern in self.patterns.iter() {
            if !patterns.contains_key(pattern) {
                return Err(format!(
                    "pattern {} is missing, because it's in the filter {}.{}",
                    pattern.name, self.stream_name, self.name
                ));
            }
        }

        Ok(patterns.into_values().collect())
    }

    /// Filters [`Filter`]'s [`Action`]s according to its [`Pattern`]s [`PatternType`]
    /// and those of the given [`Match`]
    pub fn filtered_actions_from_match(&self, m: &Match) -> Vec<&Action> {
        let ip_type = if self.has_ip {
            self.patterns
                .iter()
                .zip(m)
                .find(|(p, _)| p.pattern_type() == PatternType::Ip)
                .map(|(_, m)| -> _ {
                    // Using this dumb heuristic is ok,
                    // because we know we have a valid IP address.
                    if m.contains(':') {
                        PatternType::Ipv6
                    } else if m.contains('.') {
                        PatternType::Ipv4
                    } else {
                        // This else should not happen, but better falling back on something than
                        // panicking, right? Maybe we should add a warning there?
                        PatternType::Regex
                    }
                })
                .unwrap_or(PatternType::Regex)
        } else {
            PatternType::Regex
        };

        let mut actions: Vec<_> = self
            .actions
            .values()
            // If specific ip version, check it
            .filter(move |action| !action.ipv4only || ip_type == PatternType::Ipv4)
            .filter(move |action| !action.ipv6only || ip_type == PatternType::Ipv6)
            .collect();

        // Sort by after
        actions.sort_by(|a, b| {
            a.after_duration
                .unwrap_or_default()
                .cmp(&b.after_duration.unwrap_or_default())
        });
        actions
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

// Used by FilterMethods to insert Patterns into a user regex
fn compute_regex_with_patterns(
    mut regex: String,
    config_patterns: &Patterns,
) -> Result<(String, Regex, BTreeSet<Arc<Pattern>>), String> {
    let mut new_patterns = BTreeSet::new();
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
            new_patterns.insert(pattern.clone());
        }
        regex = regex.replacen(pattern.name_with_braces(), &pattern.regex, 1);
    }
    let compiled = Regex::new(&regex).map_err(|err| err.to_string())?;
    Ok((regex, compiled, new_patterns))
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
            actions: actions.into_iter().map(|a| (a.name.clone(), a)).collect(),
            option_regex: Some(FilterMethodRegex {
                regex: regex.into_iter().map(|s| s.into()).collect(),
                ..Default::default()
            }),
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
impl Filter {
    pub fn regex(&mut self) -> &mut FilterMethodRegex {
        if self.option_regex.is_none() {
            self.option_regex = Some(FilterMethodRegex::default());
        }
        self.option_regex.as_mut().unwrap()
    }
}

#[cfg(test)]
pub mod tests {
    use crate::concepts::action::tests::{ok_action, ok_action_with_after};
    use crate::concepts::pattern::PatternIp;
    use crate::concepts::pattern::tests::{
        boubou_pattern_with_ignore, number_pattern, ok_pattern_with_ignore,
    };

    use super::*;

    pub fn ok_filter() -> Filter {
        let mut filter = Filter::default();
        let name = "name".to_string();
        filter.regex().regex = vec!["reg".into()];
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
        filter.regex().regex = Vec::new();
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

    // todo split, let get_match_from_patterns_here & move get_match in as_regex.rs
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
        filter.regex().regex.push("insert <name> here$".to_string());
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
        assert!(
            filter
                .get_match_from_patterns(BTreeMap::from([(pattern.clone(), "abc".into())]))
                .is_err()
        );
        // Ignored match
        assert!(
            filter
                .get_match_from_patterns(BTreeMap::from([(pattern.clone(), "a".into())]))
                .is_err()
        );
        // Bad pattern
        assert!(
            filter
                .get_match_from_patterns(BTreeMap::from([(boubou.clone(), "bou".into())]))
                .is_err()
        );
        // Bad number of patterns
        assert!(
            filter
                .get_match_from_patterns(BTreeMap::from([
                    (pattern.clone(), "b".into()),
                    (boubou.clone(), "bou".into()),
                ]))
                .is_err()
        );
        // Bad number of patterns
        assert!(filter.get_match_from_patterns(BTreeMap::from([])).is_err());

        // two patterns in one regex
        filter = Filter::default();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex()
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
        assert!(
            filter
                .get_match_from_patterns(BTreeMap::from([
                    (pattern.clone(), "abc".into()),
                    (boubou.clone(), "bou".into()),
                ]))
                .is_err()
        );
        // Ignored match
        assert!(
            filter
                .get_match_from_patterns(BTreeMap::from([
                    (pattern.clone(), "b".into()),
                    (boubou.clone(), "boubou".into()),
                ]))
                .is_err()
        );
        // Bad pattern
        assert!(
            filter
                .get_match_from_patterns(BTreeMap::from([
                    (pattern.clone(), "b".into()),
                    (number_pattern.clone(), "1".into()),
                ]))
                .is_err()
        );
        // Bad number of patterns
        assert!(
            filter
                .get_match_from_patterns(BTreeMap::from([
                    (pattern.clone(), "b".into()),
                    (boubou.clone(), "bou".into()),
                    (number_pattern.clone(), "1".into()),
                ]))
                .is_err()
        );
        // Bad number of patterns
        assert!(filter.get_match_from_patterns(BTreeMap::from([])).is_err());

        // multiple regexes with same pattern
        filter = Filter::default();
        filter.actions.insert(name.clone(), ok_action());
        filter.regex().regex.push("insert <name> here".to_string());
        filter
            .regex()
            .regex
            .push("also add <name> there".to_string());
        assert!(filter.setup(&name, &name, &patterns).is_ok());
        assert_eq!(filter.get_match("insert a here"), None);
        assert_eq!(filter.get_match("insert b here"), Some(vec!("b".into())));
        assert_eq!(filter.get_match("also add a there"), None);
        assert_eq!(filter.get_match("also add b there"), Some(vec!("b".into())));

        // multiple regexes with same patterns
        filter = Filter::default();
        filter.actions.insert(name.clone(), ok_action());
        filter
            .regex()
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        filter
            .regex()
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

    #[test]
    fn filtered_actions_from_match_one_regex_pattern() {
        let az_patterns = Pattern::new_map("az", "[a-z]+").unwrap();
        let action = Action::new(
            vec!["zblorg <az>"],
            None,
            false,
            "test",
            "test",
            "a1",
            &az_patterns,
            0,
        );
        let filter = Filter::new(
            vec![action.clone()],
            vec![""],
            None,
            None,
            "test",
            "test",
            Duplicate::default(),
            &az_patterns,
        );
        assert_eq!(
            vec![&action],
            filter.filtered_actions_from_match(&vec!["zboum".into()])
        );
    }

    #[test]
    fn filtered_actions_from_match_two_regex_patterns() {
        let patterns = BTreeMap::from([
            (
                "az".to_string(),
                Arc::new(Pattern::new("az", "[a-z]+").unwrap()),
            ),
            (
                "num".to_string(),
                Arc::new(Pattern::new("num", "[0-9]{1,3}").unwrap()),
            ),
        ]);
        let action1 = Action::new(
            vec!["zblorg <az> <num>"],
            None,
            false,
            "test",
            "test",
            "a1",
            &patterns,
            0,
        );
        let action2 = Action::new(
            vec!["zbleurg <num> <az>"],
            None,
            false,
            "test",
            "test",
            "a2",
            &patterns,
            0,
        );
        let filter = Filter::new(
            vec![action1.clone(), action2.clone()],
            vec![""],
            None,
            None,
            "test",
            "test",
            Duplicate::default(),
            &patterns,
        );
        assert_eq!(
            vec![&action1, &action2],
            filter.filtered_actions_from_match(&vec!["zboum".into()])
        );
    }

    #[test]
    fn filtered_actions_from_match_one_regex_one_ip() {
        let patterns = BTreeMap::from([
            (
                "az".to_string(),
                Arc::new(Pattern::new("az", "[a-z]+").unwrap()),
            ),
            ("ip".to_string(), {
                let mut pattern = Pattern {
                    ip: PatternIp {
                        pattern_type: PatternType::Ip,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                pattern.setup("ip").unwrap();
                Arc::new(pattern)
            }),
        ]);
        let action4 = Action::new(
            vec!["zblorg4 <az> <ip>"],
            None,
            false,
            "test",
            "test",
            "action4",
            &patterns,
            4,
        );
        let action6 = Action::new(
            vec!["zblorg6 <az> <ip>"],
            None,
            false,
            "test",
            "test",
            "action6",
            &patterns,
            6,
        );
        let action = Action::new(
            vec!["zblorg <az> <ip>"],
            None,
            false,
            "test",
            "test",
            "action",
            &patterns,
            0,
        );
        let filter = Filter::new(
            vec![action4.clone(), action6.clone(), action.clone()],
            vec!["<az>: <ip>"],
            None,
            None,
            "test",
            "test",
            Duplicate::default(),
            &patterns,
        );
        assert_eq!(
            filter.filtered_actions_from_match(&vec!["zboum".into(), "1.2.3.4".into()]),
            vec![&action, &action4],
        );
        assert_eq!(
            filter.filtered_actions_from_match(&vec!["zboum".into(), "ab4:35f::1".into()]),
            vec![&action, &action6],
        );
    }
}
