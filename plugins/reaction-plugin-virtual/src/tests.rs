use std::time::{SystemTime, UNIX_EPOCH};

use reaction_plugin::{Exec, PluginInfo, Value};
use serde_json::json;

use crate::Plugin;

#[tokio::test]
async fn conf_stream() {
    // Invalid type
    assert!(
        Plugin::default()
            .stream_impl("stream".into(), "virtu".into(), Value::Null)
            .await
            .is_err()
    );

    assert!(
        Plugin::default()
            .stream_impl("stream".into(), "virtual".into(), Value::Null)
            .await
            .is_ok()
    );
    eprintln!(
        "err: {:?}",
        Plugin::default()
            .stream_impl("stream".into(), "virtual".into(), json!({}).into())
            .await
    );
    assert!(
        Plugin::default()
            .stream_impl("stream".into(), "virtual".into(), json!({}).into())
            .await
            .is_ok()
    );

    // Invalid conf: must be empty
    assert!(
        Plugin::default()
            .stream_impl(
                "stream".into(),
                "virtual".into(),
                json!({"key": "value" }).into()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn conf_action() {
    let valid_conf = json!({ "send": "message", "to": "stream" });

    let missing_send_conf = json!({ "to": "stream" });
    let missing_to_conf = json!({ "send": "stream" });
    let extra_attr_conf = json!({ "send": "message", "send2": "message", "to": "stream" });

    let patterns = Vec::default();

    // Invalid type
    assert!(
        Plugin::default()
            .action_impl(
                "stream".into(),
                "filter".into(),
                "action".into(),
                "virtu".into(),
                Value::Null,
                patterns.clone()
            )
            .await
            .is_err()
    );
    assert!(
        Plugin::default()
            .action_impl(
                "stream".into(),
                "filter".into(),
                "action".into(),
                "virtual".into(),
                valid_conf.into(),
                patterns.clone()
            )
            .await
            .is_ok()
    );

    for conf in [missing_send_conf, missing_to_conf, extra_attr_conf] {
        assert!(
            Plugin::default()
                .action_impl(
                    "stream".into(),
                    "filter".into(),
                    "action".into(),
                    "virtual".into(),
                    conf.clone().into(),
                    patterns.clone()
                )
                .await
                .is_err(),
            "conf: {:?}",
            conf
        );
    }
}

#[tokio::test]
async fn conf_send() {
    // Valid to: option
    let mut plugin = Plugin::default();
    plugin
        .stream_impl("stream".into(), "virtual".into(), Value::Null)
        .await
        .unwrap();
    plugin
        .action_impl(
            "stream".into(),
            "filter".into(),
            "action".into(),
            "virtual".into(),
            json!({ "send": "message", "to": "stream" }).into(),
            Vec::default(),
        )
        .await
        .unwrap();
    assert!(plugin.finish_setup().await.is_ok());

    // Invalid to: option
    let mut plugin = Plugin::default();
    plugin
        .stream_impl("stream".into(), "virtual".into(), Value::Null)
        .await
        .unwrap();
    plugin
        .action_impl(
            "stream".into(),
            "filter".into(),
            "action".into(),
            "virtual".into(),
            json!({ "send": "message", "to": "stream1" }).into(),
            Vec::default(),
        )
        .await
        .unwrap();
    assert!(plugin.finish_setup().await.is_err());
}

// #[tokio::test]
// async fn conf_empty_stream() {
//     let mut plugin = Plugin::default();
//     plugin
//         .stream_impl("stream".into(), "virtual".into(), Value::Null)
//         .await
//         .unwrap();
//     assert!(plugin.finish_setup().await.is_err());
// }

#[tokio::test]
async fn run_simple() {
    let mut plugin = Plugin::default();
    let mut stream = plugin
        .stream_impl("stream".into(), "virtual".into(), Value::Null)
        .await
        .unwrap();
    assert!(!stream.standalone);

    let action = plugin
        .action_impl(
            "stream".into(),
            "filter".into(),
            "action".into(),
            "virtual".into(),
            json!({ "send": "message <test>", "to": "stream" }).into(),
            vec!["test".into()],
        )
        .await
        .unwrap();
    assert!(plugin.finish_setup().await.is_ok());

    for m in ["test1", "test2", "test3", "  a   a a aa a a"] {
        let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        assert!(
            action
                .tx
                .send(Exec {
                    match_: vec![m.into()],
                    time,
                })
                .await
                .is_ok()
        );
        assert_eq!(
            stream.stream.recv().await.unwrap().unwrap(),
            (format!("message {m}"), time),
        );
    }
}

#[tokio::test]
async fn run_two_actions() {
    let mut plugin = Plugin::default();
    let mut stream = plugin
        .stream_impl("stream".into(), "virtual".into(), Value::Null)
        .await
        .unwrap();
    assert!(!stream.standalone);

    let action1 = plugin
        .action_impl(
            "stream".into(),
            "filter".into(),
            "action".into(),
            "virtual".into(),
            json!({ "send": "send <a>", "to": "stream" }).into(),
            vec!["a".into(), "b".into()],
        )
        .await
        .unwrap();

    let action2 = plugin
        .action_impl(
            "stream".into(),
            "filter".into(),
            "action".into(),
            "virtual".into(),
            json!({ "send": "<b> send", "to": "stream" }).into(),
            vec!["a".into(), "b".into()],
        )
        .await
        .unwrap();

    assert!(plugin.finish_setup().await.is_ok());

    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

    assert!(
        action1
            .tx
            .send(Exec {
                match_: vec!["aa".into(), "bb".into()],
                time,
            })
            .await
            .is_ok(),
    );
    assert_eq!(
        stream.stream.recv().await.unwrap().unwrap(),
        ("send aa".into(), time),
    );

    assert!(
        action2
            .tx
            .send(Exec {
                match_: vec!["aa".into(), "bb".into()],
                time,
            })
            .await
            .is_ok(),
    );
    assert_eq!(
        stream.stream.recv().await.unwrap().unwrap(),
        ("bb send".into(), time),
    );
}
