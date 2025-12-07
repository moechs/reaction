use std::{path::Path, thread::sleep, time::Duration};

use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::predicate;
use tokio::{fs::read_to_string, runtime::Handle};

// require UDP ports 9876 & 9877 to be free on 127.0.0.1

#[tokio::test]
#[ignore = "currently failing"] // FIXME
async fn plugin_cluster() {
    // First build reaction-plugin-cluster
    Command::new("cargo")
        .args(["build", "-p", "reaction-plugin-cluster"])
        .unwrap();

    let config = read_to_string("tests/test-conf/test-cluster.jsonnet")
        .await
        .unwrap();

    let secret_key_a = "g7U1LPq2cgGSyk6CH_v1QpoXowSFKVQ8IcFljd_ZKGw=";
    let public_key_a = "HhVh7ghqpXM9375HZ82OOeB504HBSS25wgug-1vUggY=";
    let secret_key_b = "5EgRjwIpqd60IXWCGg5dFTtxkI-0fS1PlhoIhUjh1eY=";
    let public_key_b = "LPSQ9pS7m_5vvNC-fhoBNeL2-eS2Fd6aO4ImSnXp3lc=";

    let config_a = config
        .replace("PUBLIC_KEY", public_key_b)
        .replace("NODE", "A")
        .replace("1234", "9876")
        .replace("4321", "9877");
    let config_b = config
        .replace("PUBLIC_KEY", public_key_a)
        .replace("NODE", "B")
        .replace("1234", "9877")
        .replace("4321", "9876");

    let output_a = vec![
        "a0 1", "a0 2", "a0 3", "a0 4", "b0 1", "b0 2", "b0 3", "b0 4", "",
    ];
    let output_b = vec![
        "a0 1", "a0 2", "a0 3", "a0 4", "b0 1", "b0 2", "b0 3", "b0 4", "",
    ];

    let runtime = Handle::current();
    let a_handle = runtime.spawn_blocking(|| launch_node(config_a, secret_key_a, output_a));
    let b_handle = runtime.spawn_blocking(|| launch_node(config_b, secret_key_b, output_b));

    let (a_res, b_res) = tokio::join!(a_handle, b_handle);
    a_res.unwrap();
    b_res.unwrap();
}

fn launch_node(config: String, my_secret: &'static str, expected_output: Vec<&'static str>) {
    let tmp_dir = assert_fs::TempDir::new().unwrap();

    // Write node config
    tmp_dir.child("config.jsonnet").write_str(&config).unwrap();
    tmp_dir
        .child("plugin_data/cluster/secret_key_s1")
        .write_str(my_secret)
        .unwrap();

    // Copy cluster plugin
    tmp_dir
        .child("./target/debug/reaction-plugin-cluster")
        .write_file(Path::new("./target/debug/reaction-plugin-cluster"))
        .unwrap();

    sleep(Duration::from_secs(10));
    Command::cargo_bin("reaction")
        .unwrap()
        .args(["start", "--socket", "./s", "--config", "./config.jsonnet"])
        .current_dir(tmp_dir.path())
        .timeout(Duration::from_secs(5))
        // Expected exit 1: all stream exited
        .assert()
        .code(predicate::eq(1));

    // Expected output
    tmp_dir.child("log").assert(expected_output.join("\n"));
    tmp_dir.child("log").write_str("").unwrap();
}
