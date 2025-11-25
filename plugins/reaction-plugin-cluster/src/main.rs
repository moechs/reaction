use std::{
    collections::{BTreeMap, BTreeSet},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
};

use chrono::TimeDelta;
use iroh::{EndpointAddr, PublicKey, SecretKey, TransportAddr};
use reaction_plugin::{
    ActionImpl, Exec, Hello, Line, Manifest, PluginInfo, RemoteResult, StreamImpl, Value,
    main_loop, shutdown::ShutdownController, time::parse_duration,
};
use remoc::{rch::mpsc, rtc};
use serde::{Deserialize, Serialize};

mod cluster;
mod connection;
mod endpoint;
mod key;

use crate::{
    cluster::{bind, cluster_tasks},
    key::{Show, key_b64_to_bytes, secret_key},
};

#[tokio::main]
async fn main() {
    let plugin = Plugin::default();
    main_loop(plugin).await;
}

#[derive(Default)]
struct Plugin {
    streams: BTreeMap<String, StreamInit>,
    actions: BTreeMap<String, Vec<ActionInit>>,
    cluster_shutdown: ShutdownController,
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
    /// Set to `null` to use IPv4 only.
    #[serde(default = "ipv6_unspecified")]
    bind_ipv6: Option<Ipv6Addr>,
    /// Other nodes which are part of the cluster.
    nodes: Vec<NodeOption>,
    /// Max duration before we drop pending messages to a node we can't connect to.
    message_timeout: String,
}

fn ipv4_unspecified() -> Option<Ipv4Addr> {
    Some(Ipv4Addr::UNSPECIFIED)
}
fn ipv6_unspecified() -> Option<Ipv6Addr> {
    Some(Ipv6Addr::UNSPECIFIED)
}

#[derive(Serialize, Deserialize)]
struct NodeOption {
    public_key: String,
    addresses: Vec<SocketAddr>,
}

/// Stream information before start
struct StreamInit {
    name: String,
    listen_port: u16,
    bind_ipv4: Option<Ipv4Addr>,
    bind_ipv6: Option<Ipv6Addr>,
    secret_key: SecretKey,
    message_timeout: TimeDelta,
    nodes: BTreeMap<PublicKey, EndpointAddr>,
    tx: mpsc::Sender<Line>,
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
    name: String,
    send: String,
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

        let options: StreamOptions = serde_json::from_value(config.into())
            .map_err(|err| format!("invalid options: {err}"))?;

        let mut nodes = BTreeMap::default();

        let message_timeout = parse_duration(&options.message_timeout)
            .map_err(|err| format!("invalid message_timeout: {err}"))?;

        for node in options.nodes.into_iter() {
            let bytes = key_b64_to_bytes(&node.public_key)
                .map_err(|err| format!("invalid public key {}: {err}", node.public_key))?;

            let public_key = PublicKey::from_bytes(&bytes)
                .map_err(|err| format!("invalid public key {}: {err}", node.public_key))?;

            nodes.insert(
                public_key,
                EndpointAddr {
                    id: public_key,
                    addrs: node
                        .addresses
                        .into_iter()
                        .map(|addr| TransportAddr::Ip(addr))
                        .collect(),
                },
            );
        }

        let secret_key = secret_key(".", &stream_name).await?;
        eprintln!(
            "INFO public key of this node for cluster {stream_name}: {}",
            secret_key.public().show()
        );

        let (tx, rx) = mpsc::channel(1);

        let stream = StreamInit {
            name: stream_name.clone(),
            listen_port: options.listen_port,
            bind_ipv4: options.bind_ipv4,
            bind_ipv6: options.bind_ipv6,
            secret_key,
            message_timeout,
            nodes,
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
        stream_name: String,
        filter_name: String,
        action_name: String,
        action_type: String,
        config: Value,
        patterns: Vec<String>,
    ) -> RemoteResult<ActionImpl> {
        if &action_type != "cluster_send" {
            return Err("This plugin can't handle other action types than cluster".into());
        }

        let options: ActionOptions = serde_json::from_value(config.into())
            .map_err(|err| format!("invalid options: {err}"))?;

        let (tx, rx) = mpsc::channel(1);

        let init_action = ActionInit {
            name: format!("{}.{}.{}", stream_name, filter_name, action_name),
            send: options.send,
            self_: options.self_,
            patterns,
            rx,
        };

        self.actions
            .entry(options.to)
            .or_default()
            .push(init_action);

        Ok(ActionImpl { tx })
    }

    async fn finish_setup(&mut self) -> RemoteResult<()> {
        while let Some((stream_name, stream)) = self.streams.pop_first() {
            let endpoint = bind(&stream).await?;
            cluster_tasks(
                endpoint,
                stream,
                self.actions.remove(&stream_name).unwrap_or_default(),
                self.cluster_shutdown.clone(),
            );
        }
        // Check there is no action left
        if !self.actions.is_empty() {
            for (to, actions) in &self.actions {
                for action in actions {
                    eprintln!(
                        "ERROR action '{}' sends 'to' unknown stream '{}'",
                        action.name, to
                    );
                }
            }
            return Err("at least one cluster_send action has unknown 'to'".into());
        }
        // Free containers
        self.actions = Default::default();
        self.streams = Default::default();
        eprintln!("DEBUG finished setup.");

        Ok(())
    }

    async fn close(self) -> RemoteResult<()> {
        self.cluster_shutdown.ask_shutdown();
        self.cluster_shutdown.wait_all_task_shutdown().await;
        Ok(())
    }
}
