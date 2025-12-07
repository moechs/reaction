use std::{fs::read_to_string, path::Path, thread, time::Duration};

use assert_cmd::Command;
use assert_fs::prelude::*;

// require UDP ports 9876 & 9877 to be free on 127.0.0.1

#[test]
fn plugin_cluster() {
    // First build reaction-plugin-cluster
    Command::new("cargo")
        .args(["build", "-p", "reaction-plugin-cluster"])
        .unwrap();

    let config = read_to_string("tests/test-conf/test-cluster.jsonnet").unwrap();

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
        "B a0 1", "B a0 2", "B a0 3", "B a0 4", "B b0 1", "B b0 2", "B b0 3", "B b0 4", "",
    ];
    let output_b = vec![
        "A a0 1", "A a0 2", "A a0 3", "A a0 4", "A b0 1", "A b0 2", "A b0 3", "A b0 4", "",
    ];

    let a_handle = thread::spawn(|| launch_node(config_a, secret_key_a, output_a));
    let b_handle = thread::spawn(|| launch_node(config_b, secret_key_b, output_b));

    a_handle.join().unwrap();
    b_handle.join().unwrap();
}

fn launch_node(config: String, my_secret: &'static str, expected_output: Vec<&'static str>) {
    let tmp_dir = assert_fs::TempDir::new().unwrap();

    // Write node config
    tmp_dir.child("config.jsonnet").write_str(&config).unwrap();
    tmp_dir
        .child("plugin_data/cluster/secret_key_s1.txt")
        .write_str(my_secret)
        .unwrap();

    // Copy cluster plugin
    tmp_dir
        .child("./target/debug/reaction-plugin-cluster")
        .write_file(Path::new("./target/debug/reaction-plugin-cluster"))
        .unwrap();

    Command::cargo_bin("reaction")
        .unwrap()
        .args([
            "start",
            "--socket",
            "./s",
            "--config",
            "./config.jsonnet",
            "-l",
            "DEBUG",
        ])
        .current_dir(tmp_dir.path())
        .timeout(Duration::from_secs(5))
        .assert()
        .failure();

    // Expected output
    tmp_dir.child("log").assert(expected_output.join("\n"));
}
