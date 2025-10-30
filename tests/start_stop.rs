use std::{path::Path, time::Duration};

use assert_cmd::Command;
use assert_fs::{TempDir, prelude::*};
use predicates::prelude::predicate;

#[test]
fn start_stop() {
    let tmp_dir = assert_fs::TempDir::new().unwrap();

    run_reaction(&tmp_dir);

    // Expected output
    let output = [
        "start 1",
        "start 2",
        "runtime 1",
        "runtime 2",
        // no order required because they'll be awaken all together on exit
        "after",
        "after",
        "stop 1",
        "stop 2",
        "",
    ];
    tmp_dir.child("log").assert(output.join("\n"));
    tmp_dir.child("log").write_str("").unwrap();

    println!(
        "DATABASE:\n{}",
        std::fs::read_to_string(tmp_dir.child("reaction.db")).unwrap()
    );

    // Second run
    run_reaction(&tmp_dir);

    // Expected output
    // (one of them)
    let outputs = [
        [
            "start 1",
            "start 2",
            "runtime 1",
            "runtime 2",
            "runtime 1",
            "runtime 2",
            // no order required because they'll be awaken all together on exit
            "after",
            "after",
            "after",
            "after",
            "stop 1",
            "stop 2",
            "",
        ],
        [
            "start 1",
            "start 2",
            "runtime 2",
            "runtime 1",
            "runtime 1",
            "runtime 2",
            // no order required because they'll be awaken all together on exit
            "after",
            "after",
            "after",
            "after",
            "stop 1",
            "stop 2",
            "",
        ],
    ];
    let contents = std::fs::read_to_string(tmp_dir.child("log")).unwrap();
    assert!(contents == outputs[0].join("\n") || contents == outputs[1].join("\n"));
}

fn run_reaction(tmp_dir: &TempDir) {
    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/start_stop.jsonnet"))
        .unwrap();

    Command::cargo_bin("reaction")
        .unwrap()
        .args(["start", "--socket", "./s", "--config", "./config.jsonnet"])
        .current_dir(tmp_dir.path())
        .timeout(Duration::from_secs(5))
        // Expected exit 1: all stream exited
        .assert()
        .code(predicate::eq(1));
}
