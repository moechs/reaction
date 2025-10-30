use std::{error::Error, path::Path, process::Stdio, thread::sleep, time::Duration};

use assert_cmd::Command;
use assert_fs::prelude::*;
use nix::sys::signal;
use predicates::prelude::predicate;

#[test]
#[ignore = "currently failing"] // FIXME
fn actions_delayed_and_on_exit() -> Result<(), Box<dyn Error>> {
    let tmp_dir = assert_fs::TempDir::new()?;

    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/test-conf/test-after.jsonnet"))?;

    let mut cmd = Command::cargo_bin("reaction")?;
    cmd.args(["start", "--socket", "./s", "--config", "./config.jsonnet"]);
    cmd.current_dir(tmp_dir.path());
    cmd.timeout(Duration::from_secs(5));
    // Expected exit 1: all stream exited
    cmd.assert().code(predicate::eq(1));

    // Expect 9 lines of im, then de (appended after 1s), then la (appended on reaction exit).
    const EXPECTED_MATCH: usize = 9;
    const CATEGORIES: [&str; 3] = ["im", "de", "la"];
    let mut expected = String::new();
    for cat in &CATEGORIES {
        for _ in 0..EXPECTED_MATCH {
            expected += cat;
            expected += "\n";
        }
    }

    tmp_dir.child("log").assert(&expected);

    Ok(())
}

#[test]
#[ignore = "long test (~15s)"]
fn kill_stream_on_exit() -> Result<(), Box<dyn Error>> {
    let tmp_dir = assert_fs::TempDir::new()?;

    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/test-conf/test-shutdown.jsonnet"))?;

    let cmd = Command::cargo_bin("reaction")?;
    let mut cmd = std::process::Command::new(cmd.get_program());
    cmd.args(["start", "--socket", "./s", "--config", "./config.jsonnet"]);
    cmd.current_dir(tmp_dir.path());
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    let mut child = cmd.spawn()?;
    let start = std::time::Instant::now();

    // wait for reaction to start all its streams
    sleep(std::time::Duration::from_millis(500));

    let pid = nix::unistd::Pid::from_raw(child.id() as i32);

    // stop reaction, ignore kill error (should only happen if the process already exited)
    let _ = signal::kill(pid, signal::SIGINT);

    // wait for reaction exit (it waits for all streams to exit, ~15s)
    loop {
        match child.try_wait()? {
            None => {}
            Some(status) => {
                assert_eq!(
                    status.code(),
                    Some(0),
                    "Expect reaction to terminate with code 0"
                );
                break;
            }
        }

        let elapsed = std::time::Instant::now() - start;
        if elapsed > std::time::Duration::from_secs(20) {
            // try to terminate reaction before ending the test
            let _ = signal::kill(pid, signal::SIGKILL);
            let _ = child.wait();

            panic!("Test timed out");
        }
    }

    // make sure the streams were correctly signaled
    tmp_dir.child("log_term").assert("sigterm\n");
    tmp_dir.child("log_kill").assert("sigterm\n");

    Ok(())
}

#[test]
fn non_utf8_is_stripped() -> Result<(), Box<dyn Error>> {
    let tmp_dir = assert_fs::TempDir::new()?;

    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/test-conf/test-binary-input.jsonnet"))?;

    let mut cmd = Command::cargo_bin("reaction")?;
    cmd.args(["start", "--socket", "./s", "--config", "config.jsonnet"]);
    cmd.current_dir(tmp_dir.path());
    cmd.timeout(std::time::Duration::from_secs(1));
    // Expect exit code 1: all streams exited
    cmd.assert().code(predicate::eq(1));

    let expected = "received \"\x1babc \x05\"\n".repeat(3);
    tmp_dir.child("log").assert(&expected);

    Ok(())
}

#[test]
fn capture_streams_stderr() -> Result<(), Box<dyn Error>> {
    let tmp_dir = assert_fs::TempDir::new()?;

    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/test-conf/test-stream-stderr.jsonnet"))?;

    let mut cmd = Command::cargo_bin("reaction")?;
    cmd.args(["start", "--socket", "./s", "--config", "config.jsonnet"]);
    cmd.current_dir(tmp_dir.path());
    cmd.timeout(std::time::Duration::from_secs(1));
    // Expect exit code 1: all streams exited
    cmd.assert().code(predicate::eq(1));

    let mut expected = String::new();
    for n in 1..=5 {
        expected += &format!("{n}\n");
    }
    tmp_dir.child("log").assert(&expected);

    Ok(())
}

#[test]
fn manualy_trigger_filter() -> Result<(), Box<dyn Error>> {
    let tmp_dir = assert_fs::TempDir::new()?;

    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/test-conf/test-trigger.jsonnet"))?;

    // start daemon
    let cmd = Command::cargo_bin("reaction")?;
    let program = cmd.get_program();
    let mut cmd = std::process::Command::new(program);
    cmd.args(["start", "--socket", "./s", "--config", "config.jsonnet"]);
    cmd.current_dir(tmp_dir.path());
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    let mut daemon = cmd.spawn()?;
    let start = std::time::Instant::now();

    // wait for socket to be created
    loop {
        std::thread::sleep(Duration::from_millis(10));

        let c = tmp_dir.child("s");
        if c.exists() {
            break;
        }

        let elapsed = std::time::Instant::now() - start;
        if elapsed > Duration::from_secs(1) {
            let _ = daemon.kill();
            let _ = daemon.wait();
            panic!("Daemon did not create socket");
        }
    }

    let socket = tmp_dir.child("s");
    let socket_path = socket.path().to_str().unwrap();

    // trigger event manually
    let mut cmd_trigger = Command::cargo_bin("reaction")?;
    cmd_trigger.current_dir(tmp_dir.path());
    cmd_trigger.args(["trigger", "--socket", socket_path, "s1.f1", "num=95"]);
    cmd_trigger.timeout(Duration::from_secs(1));
    cmd_trigger.assert().success();

    // wait for daemon exit
    loop {
        std::thread::sleep(Duration::from_millis(100));

        if let Some(res) = daemon.try_wait()? {
            assert_eq!(
                res.code(),
                Some(1),
                "Expect exit code 1: All streams exited"
            );
            break;
        }

        let elapsed = std::time::Instant::now() - start;
        if elapsed > Duration::from_secs(2) {
            let _ = daemon.kill();
            let _ = daemon.wait();
            panic!("Daemon did not exit");
        }
    }

    tmp_dir.child("log").assert("95\n");

    Ok(())
}

#[test]
fn filter_regex_match_eol() -> Result<(), Box<dyn Error>> {
    let tmp_dir = assert_fs::TempDir::new()?;

    tmp_dir
        .child("config.jsonnet")
        .write_file(Path::new("tests/test-conf/test-eol-match.jsonnet"))?;

    let mut cmd = Command::cargo_bin("reaction")?;
    cmd.args(["start", "--socket", "./s", "--config", "config.jsonnet"]);
    cmd.current_dir(tmp_dir.path());
    cmd.timeout(std::time::Duration::from_secs(1));
    // Expect exit code 1: all streams exited
    cmd.assert().code(predicate::eq(1));

    let mut expected = String::new();
    for i in 1..=5 {
        expected += &format!("{i}\n");
    }
    tmp_dir.child("log").assert(&expected);

    Ok(())
}
