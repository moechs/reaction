use std::env::set_current_dir;

use assert_fs::TempDir;
use reaction_plugin::PluginInfo;
use serde_json::json;

use crate::{Plugin, tests::insert_secret_key};

use super::{PUBLIC_KEY_A, TEST_MUTEX, stream_ok, stream_ok_port};

#[tokio::test]
async fn conf_stream() {
    // Minimal node configuration
    let nodes = json!([{
        "public_key": PUBLIC_KEY_A,
    }]);

    // Invalid type
    assert!(
        Plugin::default()
            .stream_impl("stream".into(), "clust".into(), stream_ok().into(),)
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
                .stream_impl("stream".into(), "cluster".into(), json.into())
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
            .action_impl(
                "stream".into(),
                "filter".into(),
                "action".into(),
                "cluster_sen".into(),
                json!({
                    "send": "<p1>",
                    "to": "stream",
                })
                .into(),
                patterns.clone(),
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
            Result::is_ok as fn(&_) -> bool,
        ),
        (
            json!({
                    "send": "<p1>",
                    "to": "stream",
                    "self": true,
            }),
            Result::is_ok,
        ),
        (
            json!({
                    "send": "<p1>",
                    "to": "stream",
                    "self": false,
            }),
            Result::is_ok,
        ),
        (
            // missing to
            json!({
                    "send": "<p1>",
            }),
            Result::is_err,
        ),
        (
            // missing send
            json!({
                    "to": "stream",
            }),
            Result::is_err,
        ),
        (
            // invalid self
            json!({
                    "send": "<p1>",
                    "to": "stream",
                    "self": "true",
            }),
            Result::is_err,
        ),
        (json!({}), Result::is_err),
    ] {
        assert!(is_ok(
            &Plugin::default()
                .action_impl(
                    "stream".into(),
                    "filter".into(),
                    "action".into(),
                    "cluster_send".into(),
                    json.into(),
                    patterns.clone(),
                )
                .await
        ));
    }
}

#[tokio::test]
async fn conf_send() {
    let _lock = TEST_MUTEX.lock();
    let dir = TempDir::new().unwrap();
    set_current_dir(&dir).unwrap();
    insert_secret_key().await;

    // No action is ok
    let mut plugin = Plugin::default();
    plugin
        .stream_impl(
            "stream".into(),
            "cluster".into(),
            stream_ok_port(2051).into(),
        )
        .await
        .unwrap();
    let res = plugin.finish_setup().await;
    eprintln!("{res:?}");
    assert!(res.is_ok());

    // An action is ok
    let mut plugin = Plugin::default();
    plugin
        .stream_impl(
            "stream".into(),
            "cluster".into(),
            stream_ok_port(2052).into(),
        )
        .await
        .unwrap();
    plugin
        .action_impl(
            "stream".into(),
            "filter".into(),
            "action".into(),
            "cluster_send".into(),
            json!({ "send": "message", "to": "stream" }).into(),
            Vec::default(),
        )
        .await
        .unwrap();
    assert!(plugin.finish_setup().await.is_ok());

    // Invalid to: option
    let mut plugin = Plugin::default();
    plugin
        .stream_impl(
            "stream".into(),
            "cluster".into(),
            stream_ok_port(2053).into(),
        )
        .await
        .unwrap();
    plugin
        .action_impl(
            "stream".into(),
            "filter".into(),
            "action".into(),
            "cluster_send".into(),
            json!({ "send": "message", "to": "stream1" }).into(),
            Vec::default(),
        )
        .await
        .unwrap();
    assert!(plugin.finish_setup().await.is_err());
}
