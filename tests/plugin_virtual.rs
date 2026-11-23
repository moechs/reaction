use std::{path::Path, time::Duration};

use assert_cmd::{Command, cargo::cargo_bin_cmd};
use assert_fs::prelude::*;
use predicates::prelude::predicate;

mod common;

#[test]
fn plugin_virtual() {
    // First build reaction-plugin-virtual
    Command::new("cargo")
        .args(["build", "-p", "reaction-plugin-virtual"])
        .unwrap();

    let tmp_dir = assert_fs::TempDir::new().unwrap();
    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/test-conf/test-virtual.jsonnet"))
        .unwrap();
    common::restrict_configs(tmp_dir.path()).unwrap();

    // Copy virtual plugin
    tmp_dir
        .child("./target/debug/reaction-plugin-virtual")
        .write_file(Path::new("./target/debug/reaction-plugin-virtual"))
        .unwrap();

    cargo_bin_cmd!("reaction")
        .args(["start", "--socket", "./s", "--config", "./config.jsonnet"])
        .current_dir(tmp_dir.path())
        .timeout(Duration::from_secs(5))
        // Expected exit 1: all stream exited
        .assert()
        .code(predicate::eq(1));

    // Expected output
    let output = [
        "a0 1", "a0 2", "a0 3", "a0 4", "b0 1", "b0 2", "b0 3", "b0 4", "",
    ];
    tmp_dir.child("log").assert(output.join("\n"));
    tmp_dir.child("log").write_str("").unwrap();
}
