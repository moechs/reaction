use std::{
    collections::BTreeMap,
    net::{SocketAddrV4, SocketAddrV6},
};

use iroh::{Endpoint, EndpointAddr, EndpointId, endpoint::Connection};
use reaction_plugin::Exec;
use tokio::sync::{mpsc, oneshot};

use crate::{ActionInit, StreamInit, endpoint::EndpointManager};

pub const ALPN: [&[u8]; 1] = ["reaction_cluster_1".as_bytes()];

type ShutdownNotification = oneshot::Receiver<oneshot::Sender<()>>;

pub struct Cluster {
    stream: StreamInit,
    actions: Vec<ActionInit>,
    shutdown: ShutdownNotification,
    connections: BTreeMap<EndpointId, Connection>,
    endpoint_addr_tx: mpsc::Sender<EndpointAddr>,
    connection_rx: mpsc::Receiver<Connection>,
}

impl Cluster {
    pub async fn new(
        stream: StreamInit,
        actions: Vec<ActionInit>,
        shutdown: ShutdownNotification,
    ) -> Result<(), String> {
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

        let endpoint = builder.bind().await.map_err(|err| {
            format!(
                "Could not create socket address for cluster {}: {err}",
                stream.name
            )
        })?;

        let (endpoint_addr_tx, connection_rx) =
            EndpointManager::new(endpoint, stream.name.clone(), stream.nodes.len());

        let this = Self {
            // No connection for now
            connections: Default::default(),
            // Values passed as-is
            stream,
            actions,
            shutdown,
            endpoint_addr_tx,
            connection_rx,
        };
        tokio::spawn(async move { this.task().await });

        Ok(())
    }

    async fn task(mut self) {
        // Ask connections for all nodes
        for node in self.stream.nodes.values() {
            self.endpoint_addr_tx.send(node.clone()).await.unwrap();
        }
        let action_rx = self.spawn_actions();

        // Ok donc là il faut :
        // - Que j'ai une queue par noeud
        // - Que chaque élément de la queue puisse timeout
        // - Que j'envoie les messages de mes actions dans toutes les queues
        // - Que j'écoute les messages de mes pairs et que je les renvoie à mon stream
    }

    fn spawn_actions(&mut self) -> mpsc::Receiver<(Exec, bool)> {
        let (tx, rx) = mpsc::channel(1);
        while let Some(mut action) = self.actions.pop() {
            let tx = tx.clone();
            tokio::spawn(async move {
                while let Ok(Some(exec)) = action.rx.recv().await {
                    if let Err(err) = tx.send((exec, action.self_)).await {
                        eprintln!("ERROR while queueing action in cluster: {err}");
                        break;
                    }
                }
            });
        }
        rx
    }
}
