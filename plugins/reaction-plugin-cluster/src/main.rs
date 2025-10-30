use std::{
    collections::{BTreeMap, BTreeSet},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
};

use iroh::{Endpoint, PublicKey, SecretKey};
use reaction_plugin::{
    ActionImpl, Exec, Hello, Manifest, PluginInfo, RemoteResult, StreamImpl, main_loop,
};
use remoc::{rch::mpsc, rtc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs;

mod cluster;
mod secret_key;
use secret_key::secret_key;

use crate::secret_key::{key_b64_to_bytes, key_bytes_to_b64};

#[tokio::main]
async fn main() {
    let plugin = Plugin::default();
    main_loop(plugin).await;
}

#[derive(Default)]
struct Plugin {
    streams: BTreeMap<String, StreamInit>,
    actions: Vec<ActionInit>,
    endpoints: BTreeMap<String, Endpoint>,
}

/// Stream options as defined by the user
#[derive(Serialize, Deserialize)]
struct StreamOptions {
    /// The UDP port to open
    listen_port: u16,
    /// The IPv4 to bind to. Defaults to 0.0.0.0.
    /// Set to `null` to use IPv6 only.
    #[serde(default = "ipv4_unspecified")]
    bind_ipv4: Option<Ipv4Addr>,
    /// The IPv6 to bind to. Defaults to 0.0.0.0.
    /// Set to `null` to use IPv6 only.
    #[serde(default = "ipv6_unspecified")]
    bind_ipv6: Option<Ipv6Addr>,
    /// The secret that permits to join the cluster.
    shared_secret: Option<String>,
    /// The secret that permits to join the cluster, as a file.
    /// Beginning and ending whitespace will be trimmed.
    shared_secret_file: Option<String>,
    /// Other nodes which are part of the cluster.
    nodes: Vec<NodeOption>,
}

fn ipv4_unspecified() -> Option<Ipv4Addr> {
    Some(Ipv4Addr::UNSPECIFIED)
}
fn ipv6_unspecified() -> Option<Ipv6Addr> {
    Some(Ipv6Addr::UNSPECIFIED)
}

/// Stream information before start
struct StreamInit {
    listen_port: u16,
    bind_ipv4: Option<Ipv4Addr>,
    bind_ipv6: Option<Ipv6Addr>,
    shared_secret: String,
    secret_key: SecretKey,
    nodes: Vec<NodeInit>,
    tx: mpsc::Sender<String>,
}

#[derive(Serialize, Deserialize)]
struct NodeOption {
    public_key: String,
    addresses: Vec<SocketAddr>,
}

#[derive(Serialize, Deserialize)]
struct NodeInit {
    public_key: PublicKey,
    addresses: Vec<SocketAddr>,
}

#[derive(Serialize, Deserialize)]
struct ActionOptions {
    /// The line to send to the corresponding cluster, example: "ban \<ip\>"
    send: String,
    /// The name of the corresponding cluster, example: "my_cluster_stream"
    to: String,
    /// Whether the stream of this node also receives the line
    #[serde(default, rename = "self")]
    self_: bool,
}

#[derive(Serialize, Deserialize)]
struct ActionInit {
    send: String,
    to: String,
    self_: bool,
    patterns: Vec<String>,
    rx: mpsc::Receiver<Exec>,
}

impl PluginInfo for Plugin {
    async fn manifest(&mut self) -> Result<Manifest, rtc::CallError> {
        Ok(Manifest {
            hello: Hello::new(),
            streams: BTreeSet::from(["cluster".into()]),
            actions: BTreeSet::from(["cluster_send".into()]),
        })
    }

    async fn stream_impl(
        &mut self,
        stream_name: String,
        stream_type: String,
        config: Value,
    ) -> RemoteResult<StreamImpl> {
        if &stream_type != "cluster" {
            return Err("This plugin can't handle other stream types than cluster".into());
        }

        let options: StreamOptions =
            serde_json::from_value(config).map_err(|err| format!("invalid options: {err}"))?;

        let shared_secret = if let Some(shared_secret) = options.shared_secret {
            shared_secret
        } else if let Some(shared_secret_file) = &options.shared_secret_file {
            fs::read_to_string(shared_secret_file)
                .await
                .map_err(|err| {
                    format!("can't access shared_secret_file {shared_secret_file}: {err}")
                })?
                .trim()
                .to_owned()
        } else {
            return Err("missing shared secret: either shared_secret or shared_secret_file must be provided".into());
        };

        let mut init_nodes = Vec::default();
        for node in options.nodes.into_iter() {
            let bytes = key_b64_to_bytes(&node.public_key)
                .map_err(|err| format!("invalid public key {}: {err}", node.public_key))?;

            let public_key = PublicKey::from_bytes(&bytes)
                .map_err(|err| format!("invalid public key {}: {err}", node.public_key))?;

            init_nodes.push(NodeInit {
                public_key,
                addresses: node.addresses,
            });
        }

        let secret_key = secret_key(&stream_name).await?;
        eprintln!(
            "INFO public key of this node for cluster {stream_name}: {}",
            key_bytes_to_b64(secret_key.public().as_bytes())
        );

        let (tx, rx) = mpsc::channel(1);

        let stream = StreamInit {
            listen_port: options.listen_port,
            bind_ipv4: options.bind_ipv4,
            bind_ipv6: options.bind_ipv6,
            shared_secret,
            secret_key,
            nodes: init_nodes,
            tx,
        };

        if let Some(_) = self.streams.insert(stream_name, stream) {
            return Err("this virtual stream has already been initialized".into());
        }

        Ok(StreamImpl {
            stream: rx,
            standalone: true,
        })
    }

    async fn action_impl(
        &mut self,
        _stream_name: String,
        _filter_name: String,
        _action_name: String,
        action_type: String,
        config: Value,
        patterns: Vec<String>,
    ) -> RemoteResult<ActionImpl> {
        if &action_type != "cluster" {
            return Err("This plugin can't handle other action types than cluster".into());
        }

        let options: ActionOptions =
            serde_json::from_value(config).map_err(|err| format!("invalid options: {err}"))?;

        let (tx, rx) = mpsc::channel(1);

        let init_action = ActionInit {
            send: options.send,
            to: options.to,
            self_: options.self_,
            patterns,
            rx,
        };

        self.actions.push(init_action);

        Ok(ActionImpl { tx })
    }

    async fn finish_setup(&mut self) -> RemoteResult<()> {
        self.endpoint_init().await
    }

    async fn close(self) -> RemoteResult<()> {
        todo!()
    }
}
