use std::{
    process::{Child, Command},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reaction_plugin::{ActionConfig, Exec, PluginInfo, StreamConfig, Value};
use serde_json::json;
use tokio::process::Command as TokioCommand;

use crate::Plugin;

fn now() -> Duration {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
}

#[tokio::test]
async fn conf_stream() {
    // No stream is supported by ipset
    assert!(
        Plugin::default()
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "ipset".into(),
                    config: Value::Null
                }],
                vec![],
                vec![]
            )
            .await
            .is_err()
    );

    // Nothing is ok
    assert!(
        Plugin::default()
            .load_config(vec![], vec![], vec![])
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn conf_action_standalone() {
    let p = vec!["name".into(), "ip".into(), "ip2".into()];
    let p_noip = vec!["name".into(), "ip2".into()];

    for (is_ok, conf, patterns) in [
        // minimal set
        (true, json!({ "set": "test" }), &p),
        // missing set key
        (false, json!({}), &p),
        (false, json!({ "version": "ipv4" }), &p),
        // unknown key
        (false, json!({ "set": "test", "unknown": "yes" }), &p),
        (false, json!({ "set": "test", "ip_index": 1 }), &p),
        (false, json!({ "set": "test", "timeout_u32": 1 }), &p),
        // pattern //
        (true, json!({ "set": "test" }), &p),
        (true, json!({ "set": "test", "pattern": "ip" }), &p),
        (true, json!({ "set": "test", "pattern": "ip2" }), &p),
        (true, json!({ "set": "test", "pattern": "ip2" }), &p_noip),
        // unknown pattern "ip"
        (false, json!({ "set": "test" }), &p_noip),
        (false, json!({ "set": "test", "pattern": "ip" }), &p_noip),
        // unknown pattern
        (false, json!({ "set": "test", "pattern": "unknown" }), &p),
        (false, json!({ "set": "test", "pattern": "uwu" }), &p_noip),
        // bad type
        (false, json!({ "set": "test", "pattern": 0 }), &p_noip),
        (false, json!({ "set": "test", "pattern": true }), &p_noip),
        // action //
        (true, json!({ "set": "test", "action": "add" }), &p),
        (true, json!({ "set": "test", "action": "del" }), &p),
        // unknown action
        (false, json!({ "set": "test", "action": "create" }), &p),
        (false, json!({ "set": "test", "action": "insert" }), &p),
        (false, json!({ "set": "test", "action": "delete" }), &p),
        (false, json!({ "set": "test", "action": "destroy" }), &p),
        // bad type
        (false, json!({ "set": "test", "action": true }), &p),
        (false, json!({ "set": "test", "action": 1 }), &p),
        // ip version //
        // ok
        (true, json!({ "set": "test", "version": "ipv4" }), &p),
        (true, json!({ "set": "test", "version": "ipv6" }), &p),
        (true, json!({ "set": "test", "version": "ip" }), &p),
        // unknown version
        (false, json!({ "set": "test", "version": 4 }), &p),
        (false, json!({ "set": "test", "version": 6 }), &p),
        (false, json!({ "set": "test", "version": 46 }), &p),
        (false, json!({ "set": "test", "version": "5" }), &p),
        (false, json!({ "set": "test", "version": "ipv5" }), &p),
        (false, json!({ "set": "test", "version": "4" }), &p),
        (false, json!({ "set": "test", "version": "6" }), &p),
        (false, json!({ "set": "test", "version": "46" }), &p),
        // bad type
        (false, json!({ "set": "test", "version": true }), &p),
        // chains //
        // everything is fine really
        (true, json!({ "set": "test", "chains": [] }), &p),
        (true, json!({ "set": "test", "chains": ["INPUT"] }), &p),
        (true, json!({ "set": "test", "chains": ["FORWARD"] }), &p),
        (
            true,
            json!({ "set": "test", "chains": ["custom_chain"] }),
            &p,
        ),
        (
            true,
            json!({ "set": "test", "chains": ["INPUT", "FORWARD"] }),
            &p,
        ),
        (
            true,
            json!({
                "set": "test",
                "chains": ["INPUT", "FORWARD", "my_iptables_chain"]
            }),
            &p,
        ),
        // timeout //
        (true, json!({ "set": "test", "timeout": "1m" }), &p),
        (true, json!({ "set": "test", "timeout": "3 days" }), &p),
        // bad
        (false, json!({ "set": "test", "timeout": "3 dayz"}), &p),
        (false, json!({ "set": "test", "timeout": 12 }), &p),
        // target //
        // anything is fine too
        (true, json!({ "set": "test", "target": "DROP" }), &p),
        (true, json!({ "set": "test", "target": "ACCEPT" }), &p),
        (true, json!({ "set": "test", "target": "RETURN" }), &p),
        (true, json!({ "set": "test", "target": "custom_chain" }), &p),
        // bad
        (false, json!({ "set": "test", "target": 11 }), &p),
        (false, json!({ "set": "test", "target": ["DROP"] }), &p),
    ] {
        let res = Plugin::default()
            .load_config(
                vec![],
                vec![ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action".into(),
                    action_type: "ipset".into(),
                    config: conf.clone().into(),
                    patterns: patterns.clone(),
                }],
                vec![],
            )
            .await;

        assert!(
            res.is_ok() == is_ok,
            "conf: {:?}, must be ok: {is_ok}, result: {:?}",
            conf,
            // empty Result::Ok because ActionImpl is not Debug
            res.map(drop)
        );
    }
}

#[tokio::test]
async fn conf_action_merge() {
    let mut plugin = Plugin::default();

    let set1 = ActionConfig {
        stream_name: "stream".into(),
        filter_name: "filter".into(),
        action_name: "action1".into(),
        action_type: "ipset".into(),
        config: json!({
            "set": "test",
            "target": "DROP",
            "chains": ["INPUT"],
            "action": "add",
        })
        .into(),
        patterns: vec!["ip".into()],
    };

    let set2 = ActionConfig {
        stream_name: "stream".into(),
        filter_name: "filter".into(),
        action_name: "action2".into(),
        action_type: "ipset".into(),
        config: json!({
            "set": "test",
            "target": "DROP",
            "version": "ip",
            "action": "add",
        })
        .into(),
        patterns: vec!["ip".into()],
    };

    let set3 = ActionConfig {
        stream_name: "stream".into(),
        filter_name: "filter".into(),
        action_name: "action2".into(),
        action_type: "ipset".into(),
        config: json!({
            "set": "test",
            "action": "del",
        })
        .into(),
        patterns: vec!["ip".into()],
    };

    let res = plugin
        .load_config(
            vec![],
            vec![
                // First set
                set1.clone(),
                // Same set, adding options, no conflict
                set2.clone(),
                // Same set, no new options, no conflict
                set3.clone(),
                // Unrelated set, so no conflict
                ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action3".into(),
                    action_type: "ipset".into(),
                    config: json!({
                        "set": "test2",
                        "target": "target1",
                        "version": "ipv6",
                    })
                    .into(),
                    patterns: vec!["ip".into()],
                },
            ],
            vec![],
        )
        .await;

    assert!(res.is_ok(), "res: {:?}", res.map(drop));

    // Another set with conflict is not ok
    let res = plugin
        .load_config(
            vec![],
            vec![
                // First set
                set1,
                // Same set, adding options, no conflict
                set2,
                // Same set, no new options, no conflict
                set3,
                // Another set with conflict
                ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action3".into(),
                    action_type: "ipset".into(),
                    config: json!({
                        "set": "test",
                        "target": "target2",
                        "action": "del",
                    })
                    .into(),
                    patterns: vec!["ip".into()],
                },
            ],
            vec![],
        )
        .await;
    assert!(res.is_err(), "res: {:?}", res.map(drop));
}

async fn run_command(cmd: &str, args: &[&str]) -> (Option<i32>, String) {
    let cmd_stdout = TokioCommand::new(cmd).args(args).output().await.unwrap();
    (
        cmd_stdout.status.code(),
        String::from_utf8(cmd_stdout.stdout).unwrap(),
    )
}

fn ipset_list() -> impl Future<Output = (Option<i32>, String)> {
    run_command("ipset", &["list"])
}
#[allow(non_snake_case)]
fn iptables_nL() -> impl Future<Output = (Option<i32>, String)> {
    run_command("iptables", &["-n", "-L", "INPUT"])
}
#[allow(non_snake_case)]
fn ip6tables_nL() -> impl Future<Output = (Option<i32>, String)> {
    run_command("ip6tables", &["-n", "-L", "INPUT"])
}

struct Cleanup {}
impl Drop for Cleanup {
    fn drop(&mut self) {
        eprintln!("iptables cleanup test...");
        if let Ok(output) = Command::new("iptables").args(["-n", "-L"]).output()
            && String::from_utf8(output.stdout).is_ok_and(|o| o.contains("testv4"))
        {
            eprintln!("iptables cleanup...");

            let wait = |handles: Vec<std::io::Result<Child>>| {
                for handle in handles {
                    if let Ok(mut handle) = handle {
                        let _ = handle.wait();
                    }
                }
            };

            let input = ["-D", "INPUT", "1"];
            let forward = ["-D", "FORWARD", "1"];

            wait(vec![
                Command::new("iptables").args(input).spawn(),
                Command::new("iptables").args(forward).spawn(),
                Command::new("ip6tables").args(input).spawn(),
                Command::new("ip6tables").args(forward).spawn(),
            ]);
            wait(vec![
                Command::new("ipset").args(["destroy", "testv4"]).spawn(),
                Command::new("ipset").args(["destroy", "testv6"]).spawn(),
            ]);
        }
    }
}

// Needs root. Run with:
// ```
// cargo test -p reaction-plugin-ipset --no-run
// sudo ./target/debug/deps/reaction_plugin_ipset-* --ignored
// ```
#[ignore = "needs CAP_NET_ADMIN to run"]
#[tokio::test]
async fn integration_test() {
    // First cleanup
    {
        let _cleanup = Cleanup {};
    }
    // Second cleanup on panic
    let _cleanup = Cleanup {};

    let mut plugin = Plugin::default();
    let (_streams, actions) = plugin
        .load_config(
            vec![],
            vec![
                ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action1".into(),
                    action_type: "ipset".into(),
                    config: json!({
                        "set": "test",
                        "action": "add",
                    })
                    .into(),
                    patterns: vec!["ip".into()],
                },
                ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action2".into(),
                    action_type: "ipset".into(),
                    config: json!({
                        "set": "test",
                        "action": "del",
                    })
                    .into(),
                    patterns: vec!["ip".into()],
                },
            ],
            vec![],
        )
        .await
        .unwrap();

    // Nothing created for now
    let ipset = ipset_list().await;
    assert_eq!(ipset.0, Some(0));
    assert!(!ipset.1.contains("testv4"));
    assert!(!ipset.1.contains("testv6"));

    let iptables = iptables_nL().await;
    assert_eq!(ipset.0, Some(0));
    assert!(!iptables.1.contains("testv4"));

    let ip6tables = ip6tables_nL().await;
    assert_eq!(ipset.0, Some(0));
    assert!(!ip6tables.1.contains("testv6"));

    plugin.start().await.unwrap();

    // Rules inserted
    let ipset = ipset_list().await;
    assert_eq!(ipset.0, Some(0));
    assert!(ipset.1.contains("testv4"));
    assert!(ipset.1.contains("testv6"));

    let iptables = iptables_nL().await;
    assert_eq!(ipset.0, Some(0));
    assert!(iptables.1.contains("testv4"));

    let ip6tables = ip6tables_nL().await;
    assert_eq!(ipset.0, Some(0));
    assert!(ip6tables.1.contains("testv6"));

    let test_add_remove = async |ip: &str| {
        actions[0]
            .tx
            .send(Exec {
                match_: vec![ip.into()],
                time: now(),
            })
            .await
            .unwrap();

        // sleep(Duration::from_millis(100)).await;
        // eprintln!(
        //     "ipset list after {ip} insertion:\n{}",
        //     Command::new("ipset")
        //         .args(["list"])
        //         .output()
        //         .map(|o| String::from_utf8(o.stdout).unwrap_or_default())
        //         .unwrap_or_default()
        // );

        // IPs inserted
        let ipset = ipset_list().await;
        assert_eq!(ipset.0, Some(0), "check {ip} insertion");
        assert!(ipset.1.contains(ip), "check {ip} insertion");

        actions[1]
            .tx
            .send(Exec {
                match_: vec![ip.into()],
                time: now(),
            })
            .await
            .unwrap();

        // IPs removed
        let ipset = ipset_list().await;
        assert_eq!(ipset.0, Some(0), "check {ip} deletion");
        assert!(!ipset.1.contains(ip), "check {ip} deletion");
    };

    // single IPv4
    test_add_remove("198.51.100.12").await;

    // /24 IPv4 network
    test_add_remove("198.51.100.0/24").await;

    // single IPv6
    test_add_remove("2001:db8:3:4:5:6:7:8").await;

    // /56 IPv6 network
    test_add_remove("2001:db8:4c4b::/48").await;

    plugin.close().await.unwrap();

    // Rules removed
    let ipset = ipset_list().await;
    assert_eq!(ipset.0, Some(0));
    assert!(!ipset.1.contains("testv4"));
    assert!(!ipset.1.contains("testv6"));

    let iptables = iptables_nL().await;
    assert_eq!(ipset.0, Some(0));
    assert!(!iptables.1.contains("testv4"));

    let ip6tables = ip6tables_nL().await;
    assert_eq!(ipset.0, Some(0));
    assert!(!ip6tables.1.contains("testv6"));
}
