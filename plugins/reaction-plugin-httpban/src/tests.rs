use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reaction_plugin::{ActionConfig, Exec, PluginInfo};
use serde_json::json;

use crate::MyPlugin;

fn now() -> Duration {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
}

#[tokio::test]
async fn test() {
    let mut plugin = MyPlugin::default();
    let (_streams, actions) = plugin
        .load_config(
            vec![],
            vec![
                ActionConfig {
                    stream_name: "s1".into(),
                    filter_name: "f1".into(),
                    action_name: "a1".into(),
                    action_type: "httpban".into(),
                    config: json!({
                        "add": "myset",
                    })
                    .into(),
                    patterns: vec!["ip".into()],
                },
                ActionConfig {
                    stream_name: "s1".into(),
                    filter_name: "f1".into(),
                    action_name: "a2".into(),
                    action_type: "httpban".into(),
                    config: json!({
                        "remove": "myset",
                    })
                    .into(),
                    patterns: vec!["ip".into()],
                },
            ],
            vec![
                json!({
                    "socket": {
                        "mode": "600",
                        "path": "./test.sock",
                    },
                    "domains": {
                        "exemple.fr": {
                            "case": {
                                "set": "myset",
                                "code": 202,
                                "inline": "match",
                            },
                            "default": {
                                "code": 200,
                                "inline": "nomatch",
                            },
                        },
                    },
                })
                .into(),
            ],
        )
        .await
        .unwrap();

    let add_action = actions[0].clone();
    let remove_action = actions[1].clone();

    add_action
        .tx
        .send(Exec {
            match_: vec!["1.2.3.4".into()],
            time: now(),
        })
        .await
        .map_err(|_| ())
        .unwrap();

    remove_action
        .tx
        .send(Exec {
            match_: vec!["2.3.4.5".into()],
            time: now(),
        })
        .await
        .map_err(|_| ())
        .unwrap();
}
