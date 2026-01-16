use std::{env::set_current_dir, time::Duration};

use assert_fs::TempDir;
use reaction_plugin::{ActionImpl, Exec, PluginInfo, StreamImpl};
use serde_json::json;
use tokio::{fs, time::timeout};
use treedb::time::now;

use crate::{
    Plugin,
    key::secret_key_path,
    tests::{PUBLIC_KEY_A, PUBLIC_KEY_B, SECRET_KEY_A, SECRET_KEY_B, TEST_MUTEX},
};

struct TestNode {
    public_key: &'static str,
    private_key: &'static str,
    port: u16,
}

const POOL: [TestNode; 2] = [
    TestNode {
        public_key: PUBLIC_KEY_A,
        private_key: SECRET_KEY_A,
        port: 2055,
    },
    TestNode {
        public_key: PUBLIC_KEY_B,
        private_key: SECRET_KEY_B,
        port: 2056,
    },
];

async fn stream_action(
    plugin: &mut Plugin,
    name: &str,
    index: usize,
    nodes: &[TestNode],
) -> (StreamImpl, ActionImpl) {
    let stream_name = format!("stream_{name}");
    let this_node = &nodes[index];
    let other_nodes: Vec<_> = nodes
        .iter()
        .filter(|node| node.public_key != this_node.public_key)
        .map(|node| {
            json!({
                "public_key": node.public_key,
                "addresses": [format!("[::1]:{}", node.port)]
            })
        })
        .collect();

    fs::write(secret_key_path(".", &stream_name), this_node.private_key)
        .await
        .unwrap();

    let stream = plugin
        .stream_impl(
            stream_name.clone(),
            "cluster".into(),
            json!({
                "message_timeout": "30s",
                "listen_port": this_node.port,
                "nodes": other_nodes,
            })
            .into(),
        )
        .await
        .unwrap();

    let action = plugin
        .action_impl(
            "stream".into(),
            "filter".into(),
            "action".into(),
            "cluster_send".into(),
            json!({
                "send": format!("from {name}: <test>"),
                "to": stream_name,
            })
            .into(),
            vec!["test".into()],
        )
        .await
        .unwrap();

    (stream, action)
}

#[tokio::test]
async fn two_nodes_simultaneous_startup() {
    for separate_plugin in [true /*, false */] {
        let _lock = TEST_MUTEX.lock();
        let dir = TempDir::new().unwrap();
        set_current_dir(&dir).unwrap();

        let ((mut stream_a, action_a), (mut stream_b, action_b)) = if separate_plugin {
            let mut plugin_a = Plugin::default();
            let a = stream_action(&mut plugin_a, "a", 0, &POOL).await;
            plugin_a.finish_setup().await.unwrap();

            let mut plugin_b = Plugin::default();
            let b = stream_action(&mut plugin_b, "b", 1, &POOL).await;
            plugin_b.finish_setup().await.unwrap();
            (a, b)
        } else {
            let mut plugin = Plugin::default();
            let a = stream_action(&mut plugin, "a", 0, &POOL).await;
            let b = stream_action(&mut plugin, "b", 1, &POOL).await;
            plugin.finish_setup().await.unwrap();
            (a, b)
        };

        for m in ["test1", "test2", "test3"] {
            let time = now().into();
            for (stream, action, from) in [
                (&mut stream_b, &action_a, "a"),
                (&mut stream_a, &action_b, "b"),
            ] {
                assert!(
                    action
                        .tx
                        .send(Exec {
                            match_: vec![m.into()],
                            time,
                        })
                        .await
                        .is_ok(),
                    "separate_plugin: {separate_plugin}, message: {m}, from: {from}"
                );

                let received = timeout(Duration::from_millis(5000), stream.stream.recv()).await;

                assert!(
                    received.is_ok(),
                    "separate_plugin: {separate_plugin}, message: {m}, from: {from}, did timeout"
                );
                let received = received.unwrap();
                assert!(
                    received.is_ok(),
                    "separate_plugin: {separate_plugin}, message: {m}, from: {from}, remoc receive error"
                );
                let received = received.unwrap();
                assert_eq!(
                    received,
                    Some((format!("from {from}: {m}"), time)),
                    "separate_plugin: {separate_plugin}, message: {m}, from: {from}"
                );
            }
        }
    }
}
