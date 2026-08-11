use std::{collections::BTreeMap, io::Error, path, process::Stdio};

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

    pub path: String,
    /// User options
    #[serde(default)]
    pub options: Vec<Value>,
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
    pub fn setup(&mut self, name: &str) -> Result<(), String> {
        self.name = name.to_string();

        if self.path.is_empty() {
            return Err("can't specify empty plugin path".into());
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

    pub async fn launch(&self, state_directory: &str) -> Result<Child, std::io::Error> {
        // owner check
        if self.check_root {
            let path = self.path.clone();
            let stat = fs::metadata(path).await?;

            if stat.st_uid() != 0 {
                return Err(Error::other("plugin file is not owned by root"));
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
        let plugin_working_directory = format!("{state_directory}/plugin_data/{}", self.name);
        fs::create_dir_all(&plugin_working_directory).await?;

        let mut command = if self_uid.is_some_and(|self_uid| self_uid == 0) {
            let mut command = Command::new("run0");
            // --pipe gives direct, non-emulated stdio access, for better performance.
            command.arg("--pipe");
            // run the command inside the same slice as reaction
            command.arg("--slice-inherit");

            // Make path absolute for systemd
            let full_workdir = path::absolute(&plugin_working_directory)?;
            let full_workdir = full_workdir.to_str().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidFilename,
                    format!(
                        "Could not absolutize plugin working directory {plugin_working_directory}"
                    ),
                )
            })?;
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
    }
}
