use std::{path::Path, time::Duration};

use assert_cmd::{Command, cargo::cargo_bin_cmd};
use assert_fs::prelude::*;

mod common;

#[test]
fn plugin_syslog_config_is_valid() {
    Command::new("cargo")
        .args(["build", "-p", "reaction-plugin-syslog"])
        .unwrap();

    let tmp_dir = assert_fs::TempDir::new().unwrap();
    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/test-conf/test-syslog.jsonnet"))
        .unwrap();
    common::restrict_configs(tmp_dir.path()).unwrap();

    tmp_dir
        .child("./target/debug/reaction-plugin-syslog")
        .write_file(Path::new("./target/debug/reaction-plugin-syslog"))
        .unwrap();

    cargo_bin_cmd!("reaction")
        .args(["test-config", "--config", "./config.jsonnet"])
        .current_dir(tmp_dir.path())
        .timeout(Duration::from_secs(5))
        .assert()
        .success();
}
