use reaction_plugin::{ActionConfig, PluginInfo, StreamConfig, Value};
use serde_json::json;

use crate::Plugin;

#[tokio::test]
async fn conf_stream() {
    // No stream is supported by nftables
    assert!(
        Plugin::default()
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "nftables".into(),
                    config: Value::Null
                }],
                vec![]
            )
            .await
            .is_err()
    );

    // Empty config is ok
    assert!(Plugin::default().load_config(vec![], vec![]).await.is_ok());
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
        (true, json!({ "set": "test", "action": "delete" }), &p),
        (true, json!({ "set": "test", "action": "del" }), &p),
        // unknown action
        (false, json!({ "set": "test", "action": "create" }), &p),
        (false, json!({ "set": "test", "action": "insert" }), &p),
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
        // hooks //
        // everything is fine really
        (true, json!({ "set": "test", "hooks": [] }), &p),
        (
            true,
            json!({ "set": "test", "hooks": ["input", "forward", "ingress", "prerouting", "output", "postrouting", "egress"] }),
            &p,
        ),
        (false, json!({ "set": "test", "hooks": ["INPUT"] }), &p),
        (false, json!({ "set": "test", "hooks": ["FORWARD"] }), &p),
        (
            false,
            json!({ "set": "test", "hooks": ["unknown_hook"] }),
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
        (true, json!({ "set": "test", "target": "drop" }), &p),
        (true, json!({ "set": "test", "target": "accept" }), &p),
        (true, json!({ "set": "test", "target": "return" }), &p),
        (true, json!({ "set": "test", "target": "continue" }), &p),
        // bad
        (false, json!({ "set": "test", "target": "custom" }), &p),
        (false, json!({ "set": "test", "target": "DROP" }), &p),
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
                    action_type: "nftables".into(),
                    config: conf.clone().into(),
                    patterns: patterns.clone(),
                }],
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

// TODO
#[tokio::test]
async fn conf_action_merge() {
    let mut plugin = Plugin::default();

    let set1 = ActionConfig {
        stream_name: "stream".into(),
        filter_name: "filter".into(),
        action_name: "action1".into(),
        action_type: "nftables".into(),
        config: json!({
            "set": "test",
            "target": "drop",
            "hooks": ["input"],
            "action": "add",
        })
        .into(),
        patterns: vec!["ip".into()],
    };

    let set2 = ActionConfig {
        stream_name: "stream".into(),
        filter_name: "filter".into(),
        action_name: "action2".into(),
        action_type: "nftables".into(),
        config: json!({
            "set": "test",
            "target": "drop",
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
        action_type: "nftables".into(),
        config: json!({
            "set": "test",
            "action": "delete",
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
                    action_type: "nftables".into(),
                    config: json!({
                        "set": "test2",
                        "target": "return",
                        "version": "ipv6",
                    })
                    .into(),
                    patterns: vec!["ip".into()],
                },
            ],
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
                    action_type: "nftables".into(),
                    config: json!({
                        "set": "test",
                        "target": "target2",
                        "action": "del",
                    })
                    .into(),
                    patterns: vec!["ip".into()],
                },
            ],
        )
        .await;
    assert!(res.is_err(), "res: {:?}", res.map(drop));
}
