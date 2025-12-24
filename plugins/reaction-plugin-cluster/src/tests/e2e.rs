use std::{env::set_current_dir, time::Duration};

use assert_fs::TempDir;
use reaction_plugin::{Exec, PluginInfo};
use serde_json::json;
use tokio::time::timeout;
use treedb::time::now;

use crate::{
    Plugin,
    tests::{PUBLIC_KEY_B, TEST_MUTEX},
};

#[tokio::test]
async fn two_nodes_simultaneous_startup() {
    let _lock = TEST_MUTEX.lock();
    let dir = TempDir::new().unwrap();
    set_current_dir(dir).unwrap();

    let mut plugin = Plugin::default();
    let mut stream = plugin
        .stream_impl(
            "stream".into(),
            "cluster".into(),
            json!({
                "listen_port": 2054,
                "nodes": [{
                    "public_key": PUBLIC_KEY_B,
                }]
            })
            .into(),
        )
        .await
        .unwrap();
    assert!(stream.standalone);

    let action = plugin
        .action_impl(
            "stream".into(),
            "filter".into(),
            "action".into(),
            "cluster_send".into(),
            json!({
                "send": "message <test>",
                "to": "stream",
                "self": false,
            })
            .into(),
            vec!["test".into()],
        )
        .await
        .unwrap();
    assert!(plugin.finish_setup().await.is_ok());

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
