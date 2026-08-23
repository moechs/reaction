use std::env::set_current_dir;

use assert_fs::TempDir;
use reaction_plugin::{ActionConfig, PluginInfo, StreamConfig};
use serde_json::json;

use crate::{Plugin, tests::insert_secret_key};

use super::{PUBLIC_KEY_A, TEST_MUTEX, stream_ok};

#[tokio::test]
async fn conf_stream() {
    // Minimal node configuration
    let nodes = json!([{
        "public_key": PUBLIC_KEY_A,
    }]);

    // Invalid type
    assert!(
        Plugin::default()
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "clust".into(),
                    config: stream_ok().into(),
                }],
                vec![],
                vec![]
            )
            .await
            .is_err()
    );

    for (json, is_ok) in [
        (
            json!({
               "listen_port": 2048,
               "nodes": nodes,
               "message_timeout": "30m",
            }),
            Result::is_ok as fn(&_) -> bool,
        ),
        (
            // invalid time
            json!({
               "listen_port": 2048,
               "nodes": nodes,
               "message_timeout": "30pv",
            }),
            Result::is_err,
        ),
        (
            json!({
               "listen_port": 2048,
               "bind_ipv4": "0.0.0.0",
               "nodes": nodes,
               "message_timeout": "30m",
            }),
            Result::is_ok,
        ),
        (
            json!({
                   "listen_port": 2048,
                   "bind_ipv6": "::",
                   "nodes": nodes,
                   "message_timeout": "30m",
            }),
            Result::is_ok,
        ),
        (
            json!({
                   "listen_port": 2048,
                   "bind_ipv4": "0.0.0.0",
                   "bind_ipv6": "::",
                   "nodes": nodes,
                   "message_timeout": "30m",
            }),
            Result::is_ok,
        ),
        (
            json!({
                   "listen_port": 2048,
                   "bind_ipv4": null,
                   "nodes": nodes,
                   "message_timeout": "30m",
            }),
            Result::is_ok,
        ),
        (
            json!({
                   "listen_port": 2048,
                   "bind_ipv6": null,
                   "nodes": nodes,
                   "message_timeout": "30m",
            }),
            Result::is_ok,
        ),
        (
            // No bind
            json!({
                   "listen_port": 2048,
                   "bind_ipv4": null,
                   "bind_ipv6": null,
                   "nodes": nodes,
                   "message_timeout": "30m",
            }),
            Result::is_err,
        ),
        (json!({}), Result::is_err),
    ] {
        assert!(is_ok(
            &Plugin::default()
                .load_config(
                    vec![StreamConfig {
                        stream_name: "stream".into(),
                        stream_type: "cluster".into(),
                        config: json.into(),
                    }],
                    vec![],
                    vec![],
                )
                .await
        ));
    }
}

#[tokio::test]
async fn conf_action() {
    let patterns = vec!["p1".into(), "p2".into()];

    // Invalid type
    assert!(
        Plugin::default()
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "cluster".into(),
                    config: stream_ok().into(),
                }],
                vec![ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action".into(),
                    action_type: "cluster_sen".into(),
                    config: json!({
                        "send": "<p1>",
                        "to": "stream",
                    })
                    .into(),
                    patterns: patterns.clone(),
                }],
                vec![]
            )
            .await
            .is_err()
    );

    for (json, is_ok) in [
        (
            json!({
                "send": "<p1>",
                "to": "stream",
            }),
            true,
        ),
        (
            json!({
                "send": "<p1>",
                "to": "stream",
                "self": true,
            }),
            true,
        ),
        (
            json!({
                "send": "<p1>",
                "to": "stream",
                "self": false,
            }),
            true,
        ),
        (
            // missing to
            json!({
                "send": "<p1>",
            }),
            false,
        ),
        (
            // missing send
            json!({
                "to": "stream",
            }),
            false,
        ),
        (
            // invalid self
            json!({
                "send": "<p1>",
                "to": "stream",
                "self": "true",
            }),
            false,
        ),
        (
            // missing conf
            json!({}),
            false,
        ),
    ] {
        let ret = Plugin::default()
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "cluster".into(),
                    config: stream_ok().into(),
                }],
                vec![ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action".into(),
                    action_type: "cluster_send".into(),
                    config: json.clone().into(),
                    patterns: patterns.clone(),
                }],
                vec![],
            )
            .await;

        assert!(
            ret.is_ok() == is_ok,
            "is_ok: {is_ok}, ret: {:?}, action conf: {json:?}",
            ret.map(drop)
        );
    }
}

#[tokio::test]
async fn conf_send() {
    let _lock = TEST_MUTEX.lock();
    let dir = TempDir::new().unwrap();
    set_current_dir(&dir).unwrap();
    insert_secret_key().await;

    // No action is ok
    let res = Plugin::default()
        .load_config(
            vec![StreamConfig {
                stream_name: "stream".into(),
                stream_type: "cluster".into(),
                config: stream_ok().into(),
            }],
            vec![],
            vec![],
        )
        .await;
    assert!(res.is_ok(), "{:?}", res.map(drop));

    // An action is ok
    let res = Plugin::default()
        .load_config(
            vec![StreamConfig {
                stream_name: "stream".into(),
                stream_type: "cluster".into(),
                config: stream_ok().into(),
            }],
            vec![ActionConfig {
                stream_name: "stream".into(),
                filter_name: "filter".into(),
                action_name: "action".into(),
                action_type: "cluster_send".into(),
                config: json!({ "send": "message", "to": "stream" }).into(),
                patterns: vec![],
            }],
            vec![],
        )
        .await;
    assert!(res.is_ok(), "{:?}", res.map(drop));

    // Invalid to: option
    let res = Plugin::default()
        .load_config(
            vec![StreamConfig {
                stream_name: "stream".into(),
                stream_type: "cluster".into(),
                config: stream_ok().into(),
            }],
            vec![ActionConfig {
                stream_name: "stream".into(),
                filter_name: "filter".into(),
                action_name: "action".into(),
                action_type: "cluster_send".into(),
                config: json!({ "send": "message", "to": "stream1" }).into(),
                patterns: vec![],
            }],
            vec![],
        )
        .await;
    assert!(res.is_err(), "{:?}", res.map(drop));
}
