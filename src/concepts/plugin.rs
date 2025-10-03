use std::{collections::BTreeMap, process::Stdio};

use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(Default))]
#[serde(deny_unknown_fields)]
pub struct Plugin {
    pub path: String,
    /// If empty, defaults to root
    pub file_owner: Option<String>,
    /// If empty, defaults to root
    pub exec_user: Option<String>,
    /// If empty, hash is not performed
    pub sha256: Option<String>,
    /// Option for `run0`. Do not provide User.
    pub systemd_options: Option<BTreeMap<String, Vec<String>>>,
}

// NOTE
// `run0` can be used for security customisation.
// with the --pipe option, raw stdio fd are transmitted to the underlying command, so there is no overhead.

impl Plugin {
    pub fn setup(&mut self) -> Result<(), String> {
        if self.path.is_empty() {
            return Err("can't specify empty plugin path".into());
        }
        if !self.path.starts_with("/") {
            return Err(format!("plugin paths must be absolute: {}", self.path));
        }
        Ok(())
    }

    pub fn launch(&self) -> Result<Child, std::io::Error> {
        // TODO owner check
        // TODO hash check
        // TODO run0 options
        Command::new(&self.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
    }
}
