use std::{path::Path, time::Duration};

use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::predicate;

#[test]
fn plugin_virtual() {
    let tmp_dir = assert_fs::TempDir::new().unwrap();
    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/test-conf/test-virtual.jsonnet"))
        .unwrap();

    // Copy virtual plugin
    tmp_dir
        .child("./target/debug/reaction-plugin-virtual")
        .write_file(Path::new("./target/debug/reaction-plugin-virtual"))
        .unwrap();

    Command::cargo_bin("reaction")
        .unwrap()
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
    tmp_dir.child("log").assert(&output.join("\n"));
    tmp_dir.child("log").write_str("").unwrap();
}
