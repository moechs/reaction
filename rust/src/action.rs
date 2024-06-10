use std::time::Duration;

use serde::Deserialize;

use crate::parse_duration::parse_duration;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    cmd: Vec<String>,

    // TODO one shot time deserialization
    after: Option<String>,
    #[serde(skip)]
    pub after_duration: Option<Duration>,

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
    pub fn setup(
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

        if self.name.len() == 0 {
            return Err("action name is empty".into());
        }
        if self.name.contains('.') {
            return Err("character '.' is not allowed in filter name".into());
        }

        if self.cmd.len() == 0 {
            return Err("cmd is empty".into());
        } 
        if self.cmd[0].len() == 0 {
            return Err("cmd's first item is empty".into());
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
