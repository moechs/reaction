use predicates::prelude::*;
use std::error::Error;

use assert_cmd::Command;

#[test]
fn load_conf_directory() -> Result<(), Box<dyn Error>> {
    let mut cmd = Command::cargo_bin("reaction")?;
    cmd.args([
        "test-config",
        "--verbose",
        "--config",
        "./tests/test-conf/conf-00.d",
    ]);
    cmd.assert().success().stdout(predicate::eq(
        r#"Loaded the configuration from the following files in the directory ./tests/test-conf/conf-00.d in this order:
part.json
part.jsonnet
part.yaml
part.yml

concurrency: 16
state_directory: .
patterns:
  mypat:
    regex: FLAG
start:
- - echo
  - start
stop:
- - echo
  - stop
streams:
  common:
    cmd:
    - cat
    - access.log
    filters:
      from_jsonnet:
        regex:
        - ^<mypat>
        actions:
          ban:
            cmd:
            - ban
            - <mypat>
          unban:
            cmd:
            - unban
            - <mypat>
            after: 42s
      from_yaml:
        regex:
        - ^'<mypat>'
        actions:
          print:
            cmd:
            - echo
            - <mypat>
            after: 1s

"#));
    Ok(())
}

#[test]
fn example_configs_are_equal() {
    let outputs = ["config/example.yml", "config/example.jsonnet"]
        .map(|config_path| {
            let mut cmd = Command::cargo_bin("reaction").unwrap();
            cmd.args(["test-config", "--config", config_path]);
            cmd.assert().success().get_output().stdout.clone()
        })
        .map(String::from_utf8)
        .map(Result::unwrap);
    assert_eq!(outputs[0], outputs[1]);
}
