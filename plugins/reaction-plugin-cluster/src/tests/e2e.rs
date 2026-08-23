use std::{env::set_current_dir, time::Duration};

use assert_fs::TempDir;
use reaction_plugin::{ActionConfig, Exec, PluginInfo, StreamConfig};
use serde_json::json;
use tokio::{fs, time::timeout};
use treedb::time::now;

use crate::{
    Plugin,
    key::secret_key_path,
    tests::{PUBLIC_KEY_A, PUBLIC_KEY_B, SECRET_KEY_A, SECRET_KEY_B, TEST_MUTEX},
};

#[derive(Clone)]
struct TestNode {
    public_key: &'static str,
    private_key: &'static str,
    port: u16,
}

const POOL: [TestNode; 15] = [
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
    TestNode {
        public_key: "ZjEPlIdGikV_sPIAUzO3RFUidlERJUhJ9XwNAlieuvU=",
        private_key: "SCbd8Ids3Dg9MwzyMNV1KFcUtsyRbeCp7GDmu-xXBSs=",
        port: 2057,
    },
    TestNode {
        public_key: "2FUpABLl9I6bU9a2XtWKMLDzwHfrVcNEG6K8Ix6sxWQ=",
        private_key: "F0W8nIlVmuFVpelwYH4PDaBDM0COYOyXDmBEmnHyo5s=",
        port: 2058,
    },
    TestNode {
        public_key: "qR4JDI_yyPWUBrmBbQjqfFbGP14v9dEaQVPHPOjId1o=",
        private_key: "S5pxTafNXPd_9TMT4_ERuPXlZ882UmggAHrf8Yntfqg=",
        port: 2059,
    },
    TestNode {
        public_key: "NjkPBwDO4IEOBjkcxufYtVXspJNQZ0qF6GamRq2TOB4=",
        private_key: "zM_lXiFuwTkmPuuXqIghW_J0uwq0a53L_yhM57uy_R8=",
        port: 2060,
    },
    TestNode {
        public_key: "_mgTzrlE8b_zvka3LgfD5qH2h_d3S0hcDU1WzIL6C74=",
        private_key: "6Obq7fxOXK-u-P3QB5FJvNnwXdKwP1FsVJ0555o7DXs=",
        port: 2061,
    },
    TestNode {
        public_key: "FLKxCSSjjzxH0ZWTpQ8xXcSIRutXUhIDhZimjamxO2s=",
        private_key: "pBPcJ32bt4xGZIGZDLDtj0eedg7p5DENjAwA-wM-1vk=",
        port: 2062,
    },
    TestNode {
        public_key: "yYBWzhzXO4isdPW2SzI-Sv3mcy3dUl6Kl0oFN6YpuzE=",
        private_key: "nC8F6prLAY9-86EZlfXwpOjQeghlPKf3PtT-zXsJZsA=",
        port: 2063,
    },
    TestNode {
        public_key: "QLbNxlLEUt0tieD9BX9of663gCm9WjKeqch0BIFJ3CE=",
        private_key: "JL4bKNHJMaMX_ElnaDHc6Ql74HZbovcswNOrY6fN1sU=",
        port: 2064,
    },
    TestNode {
        public_key: "2cmAmcaEFW-9val6WMoHSfTW25IxiQHes7Jwy6NqLLc=",
        private_key: "TCvfDLHLQ5RxfAs7_2Th2u1XF48ygxTLAAsUzVPBn_o=",
        port: 2065,
    },
    TestNode {
        public_key: "PfKYILyGmu0C6GFUOLw4MSLxN6gtkj0XUdvQW50A2xA=",
        private_key: "LaQgDWsXpwSQlZZXd8UEllrgpeXw9biSye4zcjLclU0=",
        port: 2066,
    },
    TestNode {
        public_key: "OQMXwPl90gr-2y-f5qZIZuVG4WEae5cc8JOB39LTNYE=",
        private_key: "blcigXzk0oeQ8J1jwYFiYHJ-pMiUqbUM4SJBlxA0MiI=",
        port: 2067,
    },
    TestNode {
        public_key: "DHpkBgnQUfpC7s4-mTfpn1_PN4dzj7hCCMF6GwO3Bus=",
        private_key: "sw7-2gPOswznF2OJHJdbfyJxdjS-P5O0lie6SdOL_08=",
        port: 2068,
    },
    TestNode {
        public_key: "odjjaYd6lL1DG8N9AXHW9LGsrKIb5IlW0KZz-rgxfXA=",
        private_key: "6JU6YHRBM_rJkuQmMaGaio_PZiyzZlTIU0qE8AHPGSE=",
        port: 2069,
    },
];

async fn stream_action(
    name: &str,
    index: usize,
    nodes: &[TestNode],
) -> (StreamConfig, ActionConfig) {
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

    (
        StreamConfig {
            stream_name: stream_name.clone(),
            stream_type: "cluster".into(),
            config: json!({
                "message_timeout": "30s",
                "listen_port": this_node.port,
                "nodes": other_nodes,
            })
            .into(),
        },
        ActionConfig {
            stream_name: "stream".into(),
            filter_name: "filter".into(),
            action_name: "action".into(),
            action_type: "cluster_send".into(),
            config: json!({
                "send": format!("from {name}: <test>"),
                "to": stream_name,
            })
            .into(),
            patterns: vec!["test".into()],
        },
    )
}

#[tokio::test]
async fn two_nodes_simultaneous_startup() {
    for separate_plugin in [true /*, false */] {
        let _lock = TEST_MUTEX.lock();
        let dir = TempDir::new().unwrap();
        set_current_dir(&dir).unwrap();

        let ((mut stream_a, action_a), (mut stream_b, action_b)) = if separate_plugin {
            let mut plugin_a = Plugin::default();
            let (sa, aa) = stream_action("a", 0, &POOL[0..2]).await;
            let (mut streams_a, mut actions_a) = plugin_a
                .load_config(vec![sa], vec![aa], vec![])
                .await
                .unwrap();
            plugin_a.start().await.unwrap();

            let mut plugin_b = Plugin::default();
            let (sb, ab) = stream_action("b", 1, &POOL[0..2]).await;
            let (mut streams_b, mut actions_b) = plugin_b
                .load_config(vec![sb], vec![ab], vec![])
                .await
                .unwrap();
            plugin_b.start().await.unwrap();
            (
                (streams_a.remove(0), actions_a.remove(0)),
                (streams_b.remove(0), actions_b.remove(0)),
            )
        } else {
            let mut plugin = Plugin::default();
            let a = stream_action("a", 0, &POOL[0..2]).await;
            let b = stream_action("b", 1, &POOL[0..2]).await;
            let (mut streams, mut actions) = plugin
                .load_config(vec![a.0, b.0], vec![a.1, b.1], vec![])
                .await
                .unwrap();
            plugin.start().await.unwrap();
            (
                (streams.remove(0), actions.remove(0)),
                (streams.remove(1), actions.remove(1)),
            )
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

#[tokio::test]
async fn n_nodes_simultaneous_startup() {
    let _lock = TEST_MUTEX.lock();

    // Ports can take some time to be really closed
    let mut port_delta = 0;

    for n in 3..=POOL.len() {
        println!("\nNODES: {n}\n");
        port_delta += n;
        // for n in 3..=3 {
        let dir = TempDir::new().unwrap();
        set_current_dir(&dir).unwrap();

        let mut plugins = Vec::with_capacity(n);
        let mut streams = Vec::with_capacity(n);
        let mut actions = Vec::with_capacity(n);
        for i in 0..n {
            let mut plugin = Plugin::default();
            let name = format!("n{i}");
            let (stream, action) = stream_action(
                &name,
                i,
                &POOL[0..n]
                    .iter()
                    .map(|node| node.clone())
                    .map(|node| TestNode {
                        port: node.port + port_delta as u16,
                        ..node
                    })
                    .collect::<Vec<_>>()
                    .as_slice(),
            )
            .await;
            let (mut stream, mut action) = plugin
                .load_config(vec![stream], vec![action], vec![])
                .await
                .unwrap();
            plugin.start().await.unwrap();
            plugins.push(plugin);
            streams.push(stream.pop().unwrap());
            actions.push((action.pop().unwrap(), name));
        }

        for m in ["test1", "test2", "test3", "test4", "test5"] {
            let time = now().into();
            for (i, (action, from)) in actions.iter().enumerate() {
                assert!(
                    action
                        .tx
                        .send(Exec {
                            match_: vec![m.into()],
                            time,
                        })
                        .await
                        .is_ok(),
                    "n nodes: {n}, n°action{i}, message: {m}, from: {from}"
                );

                for (j, stream) in streams.iter_mut().enumerate().filter(|(j, _)| *j != i) {
                    let received = timeout(Duration::from_millis(5000), stream.stream.recv()).await;

                    assert!(
                        received.is_ok(),
                        "n nodes: {n}, n°action: {i}, n°stream: {j}, message: {m}, from: {from}, did timeout"
                    );
                    let received = received.unwrap();
                    assert!(
                        received.is_ok(),
                        "n nodes: {n}, n°action: {i}, n°stream: {j}, message: {m}, from: {from}, remoc receive error"
                    );
                    let received = received.unwrap();
                    assert_eq!(
                        received,
                        Some((format!("from {from}: {m}"), time)),
                        "n nodes: {n}, n°action: {i}, n°stream: {j}, message: {m}, from: {from}"
                    );
                    println!(
                        "n nodes: {n}, n°action: {i}, n°stream: {j}, message: {m}, from: {from}"
                    );
                }
            }
        }

        for plugin in plugins {
            plugin.close().await.unwrap();
        }
    }
}

// TODO test:
// with inexisting nodes
// different startup times
// stopping & restarting a node mid exchange
