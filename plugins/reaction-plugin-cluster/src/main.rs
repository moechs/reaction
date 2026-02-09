use std::{
    collections::{BTreeMap, BTreeSet},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use iroh::{EndpointAddr, PublicKey, SecretKey, TransportAddr};
use reaction_plugin::{
    ActionConfig, ActionImpl, Exec, Hello, Line, Manifest, PluginInfo, RemoteResult, StreamConfig,
    StreamImpl, line::PatternLine, main_loop, shutdown::ShutdownController, time::parse_duration,
};
use remoc::{rch::mpsc, rtc};
use serde::{Deserialize, Serialize};
use treedb::Database;

use crate::key::Show;

mod cluster;
mod connection;
mod endpoint;
mod key;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() {
    let plugin = Plugin::default();
    main_loop(plugin).await;
}

#[derive(Default)]
struct Plugin {
    init: BTreeMap<String, (StreamInit, Vec<ActionInit>)>,
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
    #[serde(default)]
    addresses: Vec<SocketAddr>,
}

/// Stream information before start
struct StreamInit {
    name: String,
    listen_port: u16,
    bind_ipv4: Option<Ipv4Addr>,
    bind_ipv6: Option<Ipv6Addr>,
    secret_key: SecretKey,
    message_timeout: Duration,
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

struct ActionInit {
    name: String,
    send: PatternLine,
    self_: bool,
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
    async fn load_config(
        &mut self,
        streams: Vec<StreamConfig>,
        actions: Vec<ActionConfig>,
    ) -> RemoteResult<(Vec<StreamImpl>, Vec<ActionImpl>)> {
        let mut ret_streams = Vec::with_capacity(streams.len());
        let mut ret_actions = Vec::with_capacity(actions.len());

        for StreamConfig {
            stream_name,
            stream_type,
            config,
        } in streams
        {
            if &stream_type != "cluster" {
                return Err("This plugin can't handle other stream types than cluster".into());
            }

            let options: StreamOptions = serde_json::from_value(config.into())
                .map_err(|err| format!("invalid options: {err}"))?;

            let mut nodes = BTreeMap::default();

            let message_timeout = parse_duration(&options.message_timeout)
                .map_err(|err| format!("invalid message_timeout: {err}"))?;

            if options.bind_ipv4.is_none() && options.bind_ipv6.is_none() {
                Err(
                    "At least one of bind_ipv4 and bind_ipv6 must be enabled. Unset at least one of them or set at least one of them to an IP.",
                )?;
            }

            if options.nodes.is_empty() {
                Err("At least one remote node has to be configured for a cluster")?;
            }

            for node in options.nodes.into_iter() {
                let bytes = key::key_b64_to_bytes(&node.public_key)
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

            let secret_key = key::secret_key(".", &stream_name).await?;
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

            if let Some(_) = self.init.insert(stream_name, (stream, vec![])) {
                return Err("this virtual stream has already been initialized".into());
            }

            ret_streams.push(StreamImpl {
                stream: rx,
                standalone: true,
            })
        }

        for ActionConfig {
            stream_name,
            filter_name,
            action_name,
            action_type,
            config,
            patterns,
        } in actions
        {
            if &action_type != "cluster_send" {
                return Err(
                    "This plugin can't handle other action types than 'cluster_send'".into(),
                );
            }

            let options: ActionOptions = serde_json::from_value(config.into())
                .map_err(|err| format!("invalid options: {err}"))?;

            let (tx, rx) = mpsc::channel(1);

            let init_action = ActionInit {
                name: format!("{}.{}.{}", stream_name, filter_name, action_name),
                send: PatternLine::new(options.send, patterns),
                self_: options.self_,
                rx,
            };

            match self.init.get_mut(&options.to) {
                Some((_, actions)) => actions.push(init_action),
                None => {
                    return Err(format!(
                        "ERROR action '{}' sends 'to' unknown stream '{}'",
                        init_action.name, options.to
                    )
                    .into());
                }
            }

            ret_actions.push(ActionImpl { tx })
        }

        Ok((ret_streams, ret_actions))
    }

    async fn start(&mut self) -> RemoteResult<()> {
        let mut db = {
            let path = PathBuf::from(".");
            let (cancellation_token, task_tracker_token) = self.cluster_shutdown.token().split();
            Database::open(&path, cancellation_token, task_tracker_token)
                .await
                .map_err(|err| format!("Can't open database: {err}"))?
        };

        while let Some((_, (stream, actions))) = self.init.pop_first() {
            let endpoint = cluster::bind(&stream).await?;
            cluster::cluster_tasks(
                endpoint,
                stream,
                actions,
                &mut db,
                self.cluster_shutdown.clone(),
            )
            .await?;
        }
        // Free containers
        self.init = Default::default();
        eprintln!("DEBUG started");

        Ok(())
    }

    async fn close(self) -> RemoteResult<()> {
        self.cluster_shutdown.ask_shutdown();
        self.cluster_shutdown.wait_all_task_shutdown().await;
        Ok(())
    }
}
