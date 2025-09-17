use std::{error::Error, path::Path, time::Duration};

use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::predicate;

#[test]
fn resume_action() -> Result<(), Box<dyn Error>> {
    let tmp_dir = assert_fs::TempDir::new()?;

    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/test-conf/test-resume-action.jsonnet"))?;

    // first run
    let mut cmd = Command::cargo_bin("reaction")?;
    cmd.args(["start", "--socket", "./s", "--config", "./config.jsonnet"]);
    cmd.current_dir(tmp_dir.path());
    cmd.timeout(Duration::from_secs(5));
    // Expected exit 1: all stream exited
    cmd.assert().code(predicate::eq(1));

    // expect a single match from the stream command
    let expected = ["starting", "start4 10.1.0.1", "stopping"].join("\n") + "\n";
    tmp_dir.child("log").assert(&expected);

    // second run, expect to resume action
    let mut cmd = Command::cargo_bin("reaction")?;
    cmd.args(["start", "--socket", "./s", "--config", "./config.jsonnet"]);
    cmd.current_dir(tmp_dir.path());
    cmd.timeout(Duration::from_secs(5));
    // Expected exit 1: all stream exited
    cmd.assert().code(predicate::eq(1));

    let expected = [
        "starting",
        "start4 10.1.0.1", // from the stream command
        "stopping",
        "starting",
        "start4 10.1.0.1", // previous action loaded from db
        "stop4 10.1.0.1",  // previous action lapses
        "start4 10.1.0.1", // from the stream command
        "stopping",
    ]
    .join("\n")
        + "\n";

    tmp_dir.child("log").assert(&expected);

    Ok(())
}
