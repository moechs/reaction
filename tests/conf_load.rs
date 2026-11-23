use std::{error::Error, path::Path};

use assert_cmd::cargo::cargo_bin_cmd;
use assert_fs::{TempDir, prelude::*};

mod common;

#[test]
fn load_conf_directory() -> Result<(), Box<dyn Error>> {
    let tmp_dir = TempDir::new()?;
    let config_dir = tmp_dir.child("conf-00.d");
    common::copy_config_tree(Path::new("./tests/test-conf/conf-00.d"), config_dir.path())?;

    let mut cmd = cargo_bin_cmd!("reaction");
    cmd.args([
        "test-config",
        "--verbose",
        "--config",
        config_dir.path().to_str().unwrap(),
    ]);
    let expected = format!(
        r#"Loaded the configuration from the following files in the directory {} in this order:
part.json
part.jsonnet
part.yaml
part.yml

concurrency: 1
patterns:
  mypat:
    regex: FLAG
    ipv4mask: null
    ipv6mask: null
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
        duplicate: extend
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
        duplicate: extend
        actions:
          print:
            cmd:
            - echo
            - <mypat>
            after: 1s

"#,
        config_dir.path().display()
    );
    cmd.assert().success().stdout(expected);
    Ok(())
}

#[test]
fn example_configs_are_equal() {
    let tmp_dir = TempDir::new().unwrap();
    let outputs = ["config/example.yml", "config/example.jsonnet"]
        .map(|config_path| {
            let dest = tmp_dir.child(Path::new(config_path).file_name().unwrap());
            common::copy_private_config(Path::new(config_path), dest.path()).unwrap();
            let mut cmd = cargo_bin_cmd!("reaction");
            cmd.args(["test-config", "--config", dest.path().to_str().unwrap()]);
            cmd.assert().success().get_output().stdout.clone()
        })
        .map(String::from_utf8)
        .map(Result::unwrap);
    assert_eq!(outputs[0], outputs[1]);
}
