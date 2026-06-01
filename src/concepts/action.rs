use std::{cmp::Ordering, collections::BTreeSet, fmt::Display, sync::Arc, time::Duration};

use reaction_plugin::{
    ActionConfig,
    time::{deser_opt_duration, ser_opt_duration},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Command;

use super::{Match, Pattern, PatternType};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    #[serde(default)]
    pub cmd: Vec<String>,

    // TODO one shot time deserialization
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deser_opt_duration",
        serialize_with = "ser_opt_duration"
    )]
    pub after: Option<Duration>,

    #[serde(
        rename = "onexit",
        default = "set_false",
        skip_serializing_if = "is_false"
    )]
    pub on_exit: bool,
    #[serde(default = "set_false", skip_serializing_if = "is_false")]
    pub oneshot: bool,

    #[serde(default = "set_false", skip_serializing_if = "is_false")]
    pub ipv4only: bool,
    #[serde(default = "set_false", skip_serializing_if = "is_false")]
    pub ipv6only: bool,

    #[serde(skip)]
    pub patterns: Arc<BTreeSet<Arc<Pattern>>>,
    #[serde(skip)]
    pub name: String,
    #[serde(skip)]
    pub filter_name: String,
    #[serde(skip)]
    pub stream_name: String,

    // Plugin-specific
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub action_type: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub options: Value,
}

fn set_false() -> bool {
    false
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Action {
    pub fn is_plugin(&self) -> bool {
        self.action_type
            .as_ref()
            .is_some_and(|action_type| action_type != "cmd")
    }

    pub fn setup(
        &mut self,
        stream_name: &str,
        filter_name: &str,
        name: &str,
        patterns: Arc<BTreeSet<Arc<Pattern>>>,
    ) -> Result<(), String> {
        self._setup(stream_name, filter_name, name, patterns)
            .map_err(|msg| format!("action {}: {}", name, msg))
    }
    fn _setup(
        &mut self,
        stream_name: &str,
        filter_name: &str,
        name: &str,
        patterns: Arc<BTreeSet<Arc<Pattern>>>,
    ) -> Result<(), String> {
        self.stream_name = stream_name.to_string();
        self.filter_name = filter_name.to_string();
        self.name = name.to_string();

        self.patterns = patterns;

        if self.name.is_empty() {
            return Err("action name is empty".into());
        }
        if self.name.contains('.') {
            return Err("character '.' is not allowed in filter name".into());
        }

        if !self.is_plugin() {
            if self.cmd.is_empty() {
                return Err("cmd is empty".into());
            }
            if self.cmd[0].is_empty() {
                return Err("cmd's first item is empty".into());
            }
            if !self.options.is_null() {
                return Err("can't define options without a plugin type".into());
            }
        } else if !self.cmd.is_empty() {
            return Err("can't define a cmd and a plugin type".into());
        }

        if self.on_exit && self.after.is_none() {
            return Err("cannot have `onexit: true`, without an `after` directive".into());
        }

        if self.ipv4only && self.ipv6only {
            return Err("cannot have `ipv4only: true` and `ipv6only: true` in one action".into());
        }
        if self
            .patterns
            .iter()
            .all(|pattern| pattern.pattern_type() != PatternType::Ip)
        {
            if self.ipv4only {
                return Err("it makes no sense to have an action with `ipv4only: true` when no pattern of type ip is defined on the filter".into());
            }
            if self.ipv6only {
                return Err("it makes no sense to have an action with `ipv6only: true` when no pattern of type ip is defined on the filter".into());
            }
        }

        Ok(())
    }

    // TODO test
    pub fn exec(&self, match_: &Match) -> Command {
        let computed_command = if self.patterns.is_empty() {
            self.cmd.clone()
        } else {
            self.cmd
                .iter()
                .map(|item| {
                    (0..match_.len())
                        .zip(self.patterns.as_ref())
                        .fold(item.clone(), |acc, (i, pattern)| {
                            acc.replace(pattern.name_with_braces(), &match_[i])
                        })
                })
                .collect()
        };
        let mut cmd = Command::new(&computed_command[0]);
        cmd.args(&computed_command[1..]);
        cmd
    }

    pub fn to_action_config(&self) -> Result<ActionConfig, String> {
        Ok(ActionConfig {
            stream_name: self.stream_name.clone(),
            filter_name: self.filter_name.clone(),
            action_name: self.name.clone(),
            action_type: self
                .action_type
                .clone()
                .ok_or_else(|| format!("action {} doesn't load a plugin. this is a bug!", self))?,
            config: self.options.clone().into(),
            patterns: self
                .patterns
                .iter()
                .map(|pattern| pattern.name.clone())
                .collect(),
        })
    }
}

impl PartialEq for Action {
    fn eq(&self, other: &Self) -> bool {
        self.stream_name == other.stream_name && self.name == other.name
    }
}
impl Eq for Action {}
impl Ord for Action {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.stream_name.cmp(&other.stream_name) {
            Ordering::Equal => match self.filter_name.cmp(&other.filter_name) {
                Ordering::Equal => self.name.cmp(&other.name),
                o => o,
            },
            o => o,
        }
    }
}
impl PartialOrd for Action {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.stream_name, self.filter_name, self.name)
    }
}

#[cfg(test)]
impl Action {
    /// Test-only constructor designed to be easy to call
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cmd: Vec<&str>,
        after: Option<&str>,
        on_exit: bool,
        stream_name: &str,
        filter_name: &str,
        name: &str,
        config_patterns: &super::Patterns,
        ip_only: u8,
    ) -> Self {
        use reaction_plugin::time::parse_duration;

        let mut action = Self {
            cmd: cmd.into_iter().map(|s| s.into()).collect(),
            after: after.map(|s| parse_duration(s).unwrap()),
            on_exit,
            ipv4only: ip_only == 4,
            ipv6only: ip_only == 6,
            ..Default::default()
        };
        action
            .setup(
                stream_name,
                filter_name,
                name,
                config_patterns
                    .clone()
                    .into_values()
                    .collect::<BTreeSet<_>>()
                    .into(),
            )
            .unwrap();
        action
    }
}

#[cfg(test)]
pub mod tests {

    use reaction_plugin::time::parse_duration;

    use super::*;

    pub fn ok_action() -> Action {
        Action {
            cmd: vec!["command".into()],
            ..Default::default()
        }
    }

    pub fn ok_action_with_after(d: &str, name: &str) -> Action {
        let mut action = Action {
            cmd: vec!["command".into()],
            after: parse_duration(d).ok(),
            ..Default::default()
        };
        action
            .setup("", "", name, Arc::new(BTreeSet::default()))
            .unwrap();
        action
    }

    #[test]
    fn missing_config() {
        let mut action;
        let name = "name".to_string();
        let patterns = Arc::new(BTreeSet::default());

        // No command
        action = Action::default();
        assert!(action.setup(&name, &name, &name, patterns.clone()).is_err());

        // No command
        action = Action::default();
        action.cmd = vec!["".into()];
        assert!(action.setup(&name, &name, &name, patterns.clone()).is_err());

        // No command
        action = Action::default();
        action.cmd = vec!["".into(), "arg1".into()];
        assert!(action.setup(&name, &name, &name, patterns.clone()).is_err());

        // command ok
        action = ok_action();
        assert!(action.setup(&name, &name, &name, patterns.clone()).is_ok());

        // command ok
        action = ok_action();
        action.cmd.push("arg1".into());
        assert!(action.setup(&name, &name, &name, patterns.clone()).is_ok());
    }
}
