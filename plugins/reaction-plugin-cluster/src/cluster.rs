use std::{
    collections::VecDeque,
    net::{SocketAddrV4, SocketAddrV6},
    sync::Arc,
};

use chrono::{DateTime, Local};
use iroh::{Endpoint, EndpointAddr, endpoint::Connection};
use tokio::sync::{mpsc, oneshot};

use crate::{ActionInit, StreamInit, endpoint::EndpointManager};

pub const ALPN: [&[u8]; 1] = ["reaction_cluster_1".as_bytes()];

type ShutdownNotification = oneshot::Receiver<oneshot::Sender<()>>;

pub async fn bind(stream: &StreamInit) -> Result<Endpoint, String> {
    let mut builder = Endpoint::builder()
        .secret_key(stream.secret_key.clone())
        .alpns(ALPN.iter().map(|slice| slice.to_vec()).collect())
        .relay_mode(iroh::RelayMode::Disabled)
        .clear_discovery();

    if let Some(ip) = stream.bind_ipv4 {
        builder = builder.bind_addr_v4(SocketAddrV4::new(ip, stream.listen_port));
    }
    if let Some(ip) = stream.bind_ipv6 {
        builder = builder.bind_addr_v6(SocketAddrV6::new(ip, stream.listen_port, 0, 0));
    }

    builder.bind().await.map_err(|err| {
        format!(
            "Could not create socket address for cluster {}: {err}",
            stream.name
        )
    })
}

pub fn cluster_tasks(
    endpoint: Endpoint,
    stream: StreamInit,
    actions: Vec<ActionInit>,
    shutdown: ShutdownNotification,
) {
    let messages_from_actions = spawn_actions(actions, stream.tx.clone());

    let (endpoint_addr_tx, connection_rx) =
        EndpointManager::new(endpoint, stream.name.clone(), stream.nodes.len());

    // TODO create ConnectionManagers and connect them to EndpointManager
}

fn spawn_actions(
    mut actions: Vec<ActionInit>,
    own_cluster_tx: remoc::rch::mpsc::Sender<String>,
) -> mpsc::Receiver<Arc<String>> {
    let (nodes_tx, nodes_rx) = mpsc::channel(1);
    while let Some(mut action) = actions.pop() {
        let nodes_tx = nodes_tx.clone();
        let own_cluster_tx = own_cluster_tx.clone();
        tokio::spawn(async move { action.serve(nodes_tx, own_cluster_tx).await });
    }
    nodes_rx
}

impl ActionInit {
    async fn serve(
        &mut self,
        nodes_tx: mpsc::Sender<Arc<String>>,
        own_stream_tx: remoc::rch::mpsc::Sender<String>,
    ) {
        while let Ok(Some(m)) = self.rx.recv().await {
            let line = if m.match_.is_empty() {
                self.send.clone()
            } else {
                (0..(m.match_.len()))
                    .zip(&self.patterns)
                    .fold(self.send.clone(), |acc, (i, pattern)| {
                        acc.replace(pattern, &m.match_[i])
                    })
            };
            if self.self_
                && let Err(err) = own_stream_tx.send(line.clone()).await
            {
                eprintln!("ERROR while queueing message to be sent to own cluster stream: {err}");
            }

            let line = Arc::new(line);
            if let Err(err) = nodes_tx.send(line).await {
                eprintln!("ERROR while queueing message to be sent to cluster nodes: {err}");
            };

            if let Err(err) = m.result.send(Ok(())) {
                eprintln!("ERROR while responding to reaction action: {err}");
            }
        }
    }
}

pub struct TimeMessage {
    pub message: Arc<String>,
    pub timeout: DateTime<Local>,
}

pub struct ConnectionManager {
    endpoint: EndpointAddr,
    // Ask the EndpointManager to connect
    ask_connection: mpsc::Sender<EndpointAddr>,
    // Our own connection (when we have one)
    connection: Option<Connection>,
    // The EndpointManager sending us a connection (whether we asked for it or not)
    connection_rx: mpsc::Receiver<Connection>,
    // Our queue of messages to send
    queue: VecDeque<TimeMessage>,
    // Messages we send from remote nodes to our own stream
    own_cluster_tx: remoc::rch::mpsc::Sender<String>,
}
