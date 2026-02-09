use std::time::{SystemTime, UNIX_EPOCH};

use reaction_plugin::{ActionConfig, Exec, PluginInfo, StreamConfig, Value};
use serde_json::json;

use crate::Plugin;

#[tokio::test]
async fn conf_stream() {
    // Invalid type
    assert!(
        Plugin::default()
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "virtu".into(),
                    config: Value::Null
                }],
                vec![]
            )
            .await
            .is_err()
    );

    assert!(
        Plugin::default()
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "virtual".into(),
                    config: Value::Null
                }],
                vec![]
            )
            .await
            .is_ok()
    );

    assert!(
        Plugin::default()
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "virtual".into(),
                    config: json!({}).into(),
                }],
                vec![]
            )
            .await
            .is_ok()
    );

    // Invalid conf: must be empty
    assert!(
        Plugin::default()
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "virtual".into(),
                    config: json!({"key": "value" }).into(),
                }],
                vec![]
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn conf_action() {
    let streams = vec![StreamConfig {
        stream_name: "stream".into(),
        stream_type: "virtual".into(),
        config: Value::Null,
    }];

    let valid_conf = json!({ "send": "message", "to": "stream" });

    let missing_send_conf = json!({ "to": "stream" });
    let missing_to_conf = json!({ "send": "stream" });
    let extra_attr_conf = json!({ "send": "message", "send2": "message", "to": "stream" });

    let patterns = Vec::default();

    // Invalid type
    assert!(
        Plugin::default()
            .load_config(
                streams.clone(),
                vec![ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action".into(),
                    action_type: "virtu".into(),
                    config: Value::Null,
                    patterns: patterns.clone(),
                }]
            )
            .await
            .is_err()
    );
    assert!(
        Plugin::default()
            .load_config(
                streams.clone(),
                vec![ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action".into(),
                    action_type: "virtual".into(),
                    config: valid_conf.into(),
                    patterns: patterns.clone()
                }]
            )
            .await
            .is_ok()
    );

    for conf in [missing_send_conf, missing_to_conf, extra_attr_conf] {
        assert!(
            Plugin::default()
                .load_config(
                    streams.clone(),
                    vec![ActionConfig {
                        stream_name: "stream".into(),
                        filter_name: "filter".into(),
                        action_name: "action".into(),
                        action_type: "virtual".into(),
                        config: conf.clone().into(),
                        patterns: patterns.clone()
                    }]
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
    assert!(
        Plugin::default()
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "virtual".into(),
                    config: Value::Null,
                }],
                vec![ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action".into(),
                    action_type: "virtual".into(),
                    config: json!({ "send": "message", "to": "stream" }).into(),
                    patterns: vec![],
                }]
            )
            .await
            .is_ok(),
    );

    // Invalid to: option
    assert!(
        Plugin::default()
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "virtual".into(),
                    config: Value::Null,
                }],
                vec![ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action".into(),
                    action_type: "virtual".into(),
                    config: json!({ "send": "message", "to": "stream1" }).into(),
                    patterns: vec![],
                }]
            )
            .await
            .is_err(),
    );
}

// Let's allow empty streams for now.
// I guess it can be useful to have manual only actions.
//
// #[tokio::test]
// async fn conf_empty_stream() {
//     assert!(
//         Plugin::default()
//             .load_config(
//                 vec![StreamConfig {
//                     stream_name: "stream".into(),
//                     stream_type: "virtual".into(),
//                     config: Value::Null,
//                 }],
//                 vec![],
//             )
//             .await
//             .is_err(),
//     );
// }

#[tokio::test]
async fn run_simple() {
    let mut plugin = Plugin::default();
    let (mut streams, mut actions) = plugin
        .load_config(
            vec![StreamConfig {
                stream_name: "stream".into(),
                stream_type: "virtual".into(),
                config: Value::Null,
            }],
            vec![ActionConfig {
                stream_name: "stream".into(),
                filter_name: "filter".into(),
                action_name: "action".into(),
                action_type: "virtual".into(),
                config: json!({ "send": "message <test>", "to": "stream" }).into(),
                patterns: vec!["test".into()],
            }],
        )
        .await
        .unwrap();

    let mut stream = streams.pop().unwrap();
    let action = actions.pop().unwrap();
    assert!(!stream.standalone);

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
    let (mut streams, mut actions) = plugin
        .load_config(
            vec![StreamConfig {
                stream_name: "stream".into(),
                stream_type: "virtual".into(),
                config: Value::Null,
            }],
            vec![
                ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action".into(),
                    action_type: "virtual".into(),
                    config: json!({ "send": "send <a>", "to": "stream" }).into(),
                    patterns: vec!["a".into(), "b".into()],
                },
                ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action".into(),
                    action_type: "virtual".into(),
                    config: json!({ "send": "<b> send", "to": "stream" }).into(),
                    patterns: vec!["a".into(), "b".into()],
                },
            ],
        )
        .await
        .unwrap();

    let mut stream = streams.pop().unwrap();
    assert!(!stream.standalone);

    let action2 = actions.pop().unwrap();
    let action1 = actions.pop().unwrap();

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
