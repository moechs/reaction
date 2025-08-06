use std::{path::Path, time::Duration};

use assert_cmd::Command;
use assert_fs::{prelude::*, TempDir};
use predicates::prelude::predicate;

#[test]
#[ignore = "currently failing"] // FIXME
fn start_stop() {
    let tmp_dir = assert_fs::TempDir::new().unwrap();

    run_reaction(&tmp_dir);

    // Expected output
    let output = [
        "start 1",
        "start 2",
        "runtime 1",
        "runtime 2",
        "after 1",
        "after 2",
        "stop 1",
        "stop 2",
        "",
    ];
    tmp_dir.child("log").assert(&output.join("\n"));
    tmp_dir.child("log").write_str("").unwrap();

    // Second run
    run_reaction(&tmp_dir);

    // Expected output
    let output = [
        "start 1",
        "start 2",
        "runtime 1",
        "runtime 2",
        "runtime 1",
        "runtime 2",
        "after 1",
        "after 2",
        "after 1",
        "after 2",
        "stop 1",
        "stop 2",
        "",
    ];
    tmp_dir.child("log").assert(&output.join("\n"));
}

fn run_reaction(tmp_dir: &TempDir) {
    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/start_stop.jsonnet"))
        .unwrap();

    let mut cmd = Command::cargo_bin("reaction").unwrap();
    cmd.args(["start", "--socket", "./s", "--config", "./config.jsonnet"]);
    cmd.current_dir(tmp_dir.path());
    cmd.timeout(Duration::from_secs(5));
    // Expected exit 1: all stream exited
    cmd.assert().code(predicate::eq(1));
}
