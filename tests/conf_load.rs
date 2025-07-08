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
