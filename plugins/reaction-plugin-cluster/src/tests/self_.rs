use std::{env::set_current_dir, time::Duration};

use assert_fs::TempDir;
use reaction_plugin::{ActionConfig, Exec, PluginInfo, StreamConfig};
use serde_json::json;
use tokio::time::timeout;
use treedb::time::now;

use crate::{Plugin, tests::insert_secret_key};

use super::{TEST_MUTEX, stream_ok_port};

#[tokio::test]
async fn run_with_self() {
    let _lock = TEST_MUTEX.lock();
    let dir = TempDir::new().unwrap();
    set_current_dir(&dir).unwrap();
    insert_secret_key().await;

    for self_ in [true, false] {
        let mut plugin = Plugin::default();
        let (mut streams, mut actions) = plugin
            .load_config(
                vec![StreamConfig {
                    stream_name: "stream".into(),
                    stream_type: "cluster".into(),
                    config: stream_ok_port(2052).into(),
                }],
                vec![ActionConfig {
                    stream_name: "stream".into(),
                    filter_name: "filter".into(),
                    action_name: "action".into(),
                    action_type: "cluster_send".into(),
                    config: json!({
                        "send": "message <test>",
                        "to": "stream",
                        "self": self_,
                    })
                    .into(),
                    patterns: vec!["test".into()],
                }],
            )
            .await
            .unwrap();

        let mut stream = streams.pop().unwrap();
        let action = actions.pop().unwrap();
        assert!(stream.standalone);
        assert!(plugin.start().await.is_ok());

        for m in ["test1", "test2", "test3", "  a   a a aa a a"] {
            let time = now().into();
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
            if self_ {
                assert_eq!(
                    stream.stream.recv().await.unwrap().unwrap(),
                    (format!("message {m}"), time),
                );
            } else {
                // Don't receive anything
                assert!(
                    timeout(Duration::from_millis(100), stream.stream.recv())
                        .await
                        .is_err()
                );
            }
        }
    }
}
