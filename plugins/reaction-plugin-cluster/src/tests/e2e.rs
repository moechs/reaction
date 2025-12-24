use std::{env::set_current_dir, time::Duration};

use assert_fs::TempDir;
use reaction_plugin::{ActionImpl, Exec, PluginInfo, RemoteResult, StreamImpl};
use serde_json::{Value, json};
use tokio::time::timeout;
use treedb::time::now;

use crate::{
    Plugin,
    tests::{PUBLIC_KEY_A, PUBLIC_KEY_B, TEST_MUTEX},
};

// TODO
// random stream name
// write secret key at correct file path
// pass a complete list of pubkey, privkey, port and generate according confs
async fn stream_action(
    plugin: &mut Plugin,
    name: &str,
    port: u16,
    nodes: Value,
) -> RemoteResult<(StreamImpl, ActionImpl)> {
    let stream = plugin
        .stream_impl(
            "stream".into(),
            "cluster".into(),
            json!({
                "listen_port": port,
                "nodes": nodes,
            })
            .into(),
        )
        .await?;

    let action = plugin
        .action_impl(
            "stream".into(),
            "filter".into(),
            "action".into(),
            "cluster_send".into(),
            json!({
                "send": format!("from {name}: <test>"),
                "to": "stream",
            })
            .into(),
            vec!["test".into()],
        )
        .await?;

    Ok((stream, action))
}

#[tokio::test]
async fn two_nodes_simultaneous_startup() {
    let _lock = TEST_MUTEX.lock();
    let dir = TempDir::new().unwrap();
    set_current_dir(dir).unwrap();

    let mut plugin_a = Plugin::default();
    let (mut stream_a, action_a) = stream_action(
        &mut plugin_a,
        "a",
        2054,
        json!([{
           "public_key": PUBLIC_KEY_B,
            "addresses": ["127.0.0.1:2055"],
        }]),
    )
    .await
    .unwrap();
    plugin_a.finish_setup().await.unwrap();

    let mut plugin_b = Plugin::default();
    let (mut stream_b, action_b) = stream_action(
        &mut plugin_b,
        "a",
        2055,
        json!([{
           "public_key": PUBLIC_KEY_A,
            "addresses": ["127.0.0.1:2054"],
        }]),
    )
    .await
    .unwrap();
    plugin_b.finish_setup().await.unwrap();

    for m in ["test1", "test2", "test3"] {
        let time = now().into();
        for (stream, action, from) in [
            (&mut stream_a, &action_b, "a"),
            (&mut stream_b, &action_a, "b"),
        ] {
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
                timeout(Duration::from_millis(100), stream.stream.recv())
                    .await
                    .unwrap()
                    .unwrap(),
                Some((format!("from {from}: {m}"), time)),
            );
        }
    }
}
