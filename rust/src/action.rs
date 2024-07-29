use std::time::Duration;

use serde::Deserialize;

use crate::parse_duration::parse_duration;

#[derive(Clone, Debug, Deserialize)]
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

fn set_false() -> bool {
    false
}

impl Action {
    pub fn name(&self) -> ActionName {
        ActionName {
            stream: self.stream_name.clone(),
            filter: self.filter_name.clone(),
            action: self.name.clone(),
        }
    }

    pub fn after_duration(&self) -> Option<Duration> {
        self.after_duration
    }

    pub fn setup(
        &mut self,
        stream_name: &str,
        filter_name: &str,
        name: &str,
    ) -> Result<(), String> {
        self._setup(stream_name, filter_name, name)
            .map_err(|msg| format!("action {}: {}", name, msg))
    }
    fn _setup(&mut self, stream_name: &str, filter_name: &str, name: &str) -> Result<(), String> {
        self.stream_name = stream_name.to_string();
        self.filter_name = filter_name.to_string();
        self.name = name.to_string();

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
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub struct ActionName {
    pub stream: String,
    pub filter: String,
    pub action: String,
}
impl ToString for ActionName {
    fn to_string(&self) -> String {
        format!("{}.{}.{}", self.stream, self.filter, self.action)
    }
}

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
        action.setup("", "", name).unwrap();
        action
    }

    #[test]
    fn missing_config() {
        let mut action;
        let name = "name".to_string();

        // No command
        action = default_action();
        assert!(action.setup(&name, &name, &name).is_err());

        // No command
        action = default_action();
        action.cmd = vec!["".into()];
        assert!(action.setup(&name, &name, &name).is_err());

        // No command
        action = default_action();
        action.cmd = vec!["".into(), "arg1".into()];
        assert!(action.setup(&name, &name, &name).is_err());

        // command ok
        action = ok_action();
        assert!(action.setup(&name, &name, &name).is_ok());

        // command ok
        action = ok_action();
        action.cmd.push("arg1".into());
        assert!(action.setup(&name, &name, &name).is_ok());
    }
}
