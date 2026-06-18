use std::{error::Error, fs::read_to_string, path::Path, time::Duration};

use assert_cmd::cargo::cargo_bin_cmd;
use assert_fs::prelude::*;
use predicates::prelude::predicate;

#[test]
fn filter_struct() -> Result<(), Box<dyn Error>> {
    let tmp_dir = assert_fs::TempDir::new()?;

    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/test-conf/struct.jsonnet"))?;

    let mut cmd = cargo_bin_cmd!("reaction");
    cmd.args(["start", "--socket", "./s", "--config", "./config.jsonnet"]);
    cmd.current_dir(tmp_dir.path());
    cmd.timeout(Duration::from_secs(3));
    // Expected exit 1: all stream exited
    cmd.assert().code(predicate::eq(1));

    let expected = "198.51.100.12\n198.51.100.15\n";
    tmp_dir.child("out.txt").assert(expected);

    let expected = "198.51.100.12\n198.51.100.13\n198.51.100.14\n198.51.100.15\n";
    tmp_dir.child("out2.txt").assert(expected);

    // Persistance test

    tmp_dir.child("out.txt").write_str("")?;
    tmp_dir.child("out2.txt").write_str("")?;

    let mut cmd = cargo_bin_cmd!("reaction");
    cmd.args(["start", "--socket", "./s", "--config", "./config.jsonnet"]);
    cmd.current_dir(tmp_dir.path());
    cmd.timeout(Duration::from_secs(3));
    // Expected exit 1: all stream exited
    cmd.assert().code(predicate::eq(1));

    let expected = [
        "198.51.100.12\n198.51.100.15\n198.51.100.12\n198.51.100.15\n",
        "198.51.100.15\n198.51.100.12\n198.51.100.12\n198.51.100.15\n",
    ];
    let output = read_to_string(tmp_dir.child("out.txt"))?;
    assert!(output == expected[0] || output == expected[1]);

    let expected = "198.51.100.12\n198.51.100.13\n198.51.100.14\n198.51.100.15\n".to_owned();
    tmp_dir.child("out2.txt").assert(expected);

    Ok(())
}
