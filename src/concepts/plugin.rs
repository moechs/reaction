use std::{
    collections::{BTreeMap, btree_map::Entry},
    path::{self, Path},
    process::Stdio,
};

#[cfg(target_os = "macos")]
use std::os::darwin::fs::MetadataExt;
#[cfg(target_os = "freebsd")]
use std::os::freebsd::fs::MetadataExt;
#[cfg(target_os = "illumos")]
use std::os::illumos::fs::MetadataExt;
#[cfg(target_os = "linux")]
use std::os::linux::fs::MetadataExt;
#[cfg(target_os = "netbsd")]
use std::os::netbsd::fs::MetadataExt;
#[cfg(target_os = "openbsd")]
use std::os::openbsd::fs::MetadataExt;
#[cfg(target_os = "solaris")]
use std::os::solaris::fs::MetadataExt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    fs,
    process::{Child, Command},
};
use tracing::{debug, warn};

use crate::concepts::merge_attrs;

// TODO commented options block execution of program,
// while developping in my home directory.
// Some options may still be useful in production environments.
fn systemd_default_options(working_directory: &str) -> BTreeMap<String, Vec<String>> {
    BTreeMap::from(
        [
            // reaction slice (does nothing if inexistent)
            ("Slice", vec!["reaction.slice"]),
            // Started in its own directory
            ("WorkingDirectory", vec![working_directory]),
            // No file access except own directory
            ("ReadWritePaths", vec![working_directory]),
            ("ReadOnlyPaths", vec!["/"]),
            ("InaccessiblePaths", vec!["/boot", "/etc"]),
            // Protect special filesystems
            ("PrivateDevices", vec!["true"]),
            ("PrivateMounts", vec!["true"]),
            ("PrivateTmp", vec!["true"]),
            // ("PrivateUsers", vec!["true"]),
            ("ProcSubset", vec!["pid"]),
            ("ProtectClock", vec!["true"]),
            ("ProtectControlGroups", vec!["true"]),
            #[cfg(not(debug_assertions))]
            ("ProtectHome", vec!["true"]),
            ("ProtectHostname", vec!["true"]),
            ("ProtectKernelLogs", vec!["true"]),
            ("ProtectKernelModules", vec!["true"]),
            ("ProtectKernelTunables", vec!["true"]),
            ("ProtectProc", vec!["invisible"]),
            ("ProtectSystem", vec!["strict"]),
            // Various Protections
            ("LockPersonality", vec!["true"]),
            ("NoNewPrivileges", vec!["true"]),
            ("AmbientCapabilities", vec![""]),
            ("CapabilityBoundingSet", vec![""]),
            // Isolate File
            ("RemoveIPC", vec!["true"]),
            ("RestrictNamespaces", vec!["true"]),
            ("RestrictSUIDSGID", vec!["true"]),
            ("SystemCallArchitectures", vec!["native"]),
            (
                "SystemCallFilter",
                vec!["@system-service", "~@privileged", "~@resources", "~@setuid"],
            ),
            // User
            // FIXME Setting another user doesn't work, because of stdio pipe permission errors
            // ("DynamicUser", vec!["true"]),
            // ("User", vec!["reaction-plugin-test"]),
            // Too restrictive
            // ("NoExecPaths", vec!["/"]),
            // ("RestrictAddressFamilies", vec![""]),
        ]
        .map(|(k, v)| (k.into(), v.into_iter().map(|v| v.into()).collect())),
    )
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(Default))]
#[serde(deny_unknown_fields)]
pub struct Plugin {
    #[serde(skip)]
    pub name: String,

    #[serde(default)]
    pub path: String,
    /// User options
    #[serde(default)]
    options: Value,
    #[serde(skip)]
    pub all_options: Vec<Value>,
    /// Check that plugin file owner is root
    #[serde(default = "_true")]
    pub check_root: bool,
    /// Enable systemd containerization
    #[serde(default = "_true")]
    pub systemd: bool,
    /// Options for `run0`
    #[serde(default)]
    pub systemd_options: BTreeMap<String, Vec<String>>,
}

fn _true() -> bool {
    true
}

// NOTE
// `run0` can be used for security customisation.
// with the --pipe option, raw stdio fd are transmitted to the underlying command, so there is no overhead.

impl Plugin {
    pub fn merge(&mut self, other: Self) -> Result<(), String> {
        // Options are merged by the plugins, so we don't impose any merging logic on them
        // But reaction_plugin::Value::merge is provided and implements NixOS' merge logic
        if !other.options.is_null() {
            self.all_options.push(other.options);
        }
        self.all_options.extend(other.all_options);

        // Simple attributes
        self.check_root = merge_attrs(self.check_root, other.check_root, true, "check_root")?;
        self.path = merge_attrs(self.path.clone(), other.path, "".into(), "path")?;
        self.systemd = merge_attrs(self.systemd, other.systemd, true, "systemd")?;

        // If systemd_options is provided multiple times, we merge values by extending the arrays
        for (key, val) in other.systemd_options.into_iter() {
            match self.systemd_options.entry(key) {
                Entry::Vacant(e) => {
                    e.insert(val);
                }
                Entry::Occupied(mut e) => {
                    e.get_mut().extend(val);
                }
            }
        }
        Ok(())
    }

    pub fn setup(&mut self, name: &str) -> Result<(), String> {
        self.name = name.to_string();

        if self.path.is_empty() {
            return Err("can't specify empty plugin path".into());
        }

        if !self.options.is_null() {
            self.all_options.push(self.options.take());
        }

        // Only when testing, make relative paths absolute
        #[cfg(debug_assertions)]
        if !self.path.starts_with("/") {
            self.path = format!(
                "{}/{}",
                std::env::current_dir()
                    .map_err(|err| format!("error on working directory: {err}"))?
                    .to_string_lossy(),
                self.path
            );
        }

        // Disallow relative paths
        if !self.path.starts_with("/") {
            return Err(format!("plugin paths must be absolute: {}", self.path));
        }
        Ok(())
    }

    /// Override default options with user-defined options, when defined.
    pub fn systemd_setup(&self, working_directory: &str) -> BTreeMap<String, Vec<String>> {
        let mut new_options = systemd_default_options(working_directory);
        for (option, value) in self.systemd_options.iter() {
            new_options.insert(option.clone(), value.clone());
        }
        new_options
    }

    pub async fn launch(&self, state_directory: &Path) -> Result<Child, String> {
        // owner check
        if self.check_root {
            let stat = fs::symlink_metadata(&self.path)
                .await
                .map_err(|err| format!("checking path {}: {err}", self.path))?;

            if !stat.is_file() || stat.st_uid() != 0 || stat.st_mode() & 0o022 != 0 {
                return Err(
                    "plugin must be a regular file owned by root and not writable by group or others"
                        .into(),
                );
            }
        }

        let self_uid = if self.systemd {
            Some(
                // Well well we want to check if we're root
                #[allow(unsafe_code)]
                unsafe {
                    nix::libc::geteuid()
                },
            )
        } else {
            None
        };

        // Create plugin working directory (also state directory)
        let plugin_working_directory = state_directory.join("plugin_data").join(self.name.clone());
        let printable_wd = plugin_working_directory.display();

        fs::create_dir_all(&plugin_working_directory)
            .await
            .map_err(|err| format!("creating directory {printable_wd}: {err}"))?;

        let mut command = if self_uid.is_some_and(|self_uid| self_uid == 0) {
            let mut command = Command::new("run0");
            // --pipe gives direct, non-emulated stdio access, for better performance.
            command.arg("--pipe");
            // run the command inside the same slice as reaction
            command.arg("--slice-inherit");

            // Make path absolute for systemd
            let full_workdir = path::absolute(&plugin_working_directory)
                .map_err(|err| format!("converting the path {printable_wd} to absolute: {err}"))?;
            let full_workdir = full_workdir
                .to_str()
                .ok_or_else(|| format!("Could not convert the path to absolute: {printable_wd}"))?;
            let merged_systemd_options = self.systemd_setup(full_workdir);
            // run0 options
            for (option, values) in merged_systemd_options.iter() {
                for value in values.iter() {
                    command.arg("--property").arg(format!("{option}={value}"));
                }
            }
            command.arg(&self.path);
            command
        } else {
            if self.systemd {
                warn!("Disabling systemd because reaction does not run as root");
            }
            let mut command = Command::new(&self.path);
            command.current_dir(plugin_working_directory);
            command
        };
        command.arg("serve");
        debug!(
            "plugin {}: running command: {:?}",
            self.name,
            command.as_std()
        );
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("RUST_BACKTRACE", "1")
            .spawn()
            .map_err(|err| format!("could not exec plugin: {err}"))
    }
}
