use std::{cmp::Ordering, collections::BTreeSet, fmt::Display, sync::Arc};

use chrono::TimeDelta;

use serde::Deserialize;
use tokio::process::Command;

use super::parse_duration;
use super::{Match, Pattern};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    cmd: Vec<String>,

    // TODO one shot time deserialization
    after: Option<String>,
    #[serde(skip)]
    after_duration: Option<TimeDelta>,

    #[serde(rename = "onexit", default = "set_false")]
    on_exit: bool,

    #[serde(skip)]
    patterns: Arc<BTreeSet<Arc<Pattern>>>,
    #[serde(skip)]
    name: String,
    #[serde(skip)]
    filter_name: String,
    #[serde(skip)]
    stream_name: String,
}

fn set_false() -> bool {
    false
}

impl Action {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn after_duration(&self) -> Option<TimeDelta> {
        self.after_duration
    }

    pub fn on_exit(&self) -> bool {
        self.on_exit
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

        if self.cmd.is_empty() {
            return Err("cmd is empty".into());
        }
        if self.cmd[0].is_empty() {
            return Err("cmd's first item is empty".into());
        }

        if let Some(after) = &self.after {
            self.after_duration = Some(
                parse_duration(after)
                    .map_err(|err| format!("failed to parse after time: {}", err))?,
            );
            self.after = None;
        } else if self.on_exit {
            return Err("cannot have `onexit: true`, without an `after` directive".into());
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

#[allow(clippy::unwrap_used)]
#[cfg(test)]
pub mod tests {

    use super::*;

    fn default_action() -> Action {
        Action {
            cmd: Vec::new(),
            name: "".into(),
            filter_name: "".into(),
            stream_name: "".into(),
            after: None,
            after_duration: None,
            on_exit: false,
            patterns: Arc::new(BTreeSet::default()),
        }
    }

    pub fn ok_action() -> Action {
        let mut action = default_action();
        action.cmd = vec!["command".into()];
        action
    }

    pub fn ok_action_with_after(d: String, name: &str) -> Action {
        let mut action = default_action();
        action.cmd = vec!["command".into()];
        action.after = Some(d);
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
        action = default_action();
        assert!(action.setup(&name, &name, &name, patterns.clone()).is_err());

        // No command
        action = default_action();
        action.cmd = vec!["".into()];
        assert!(action.setup(&name, &name, &name, patterns.clone()).is_err());

        // No command
        action = default_action();
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
