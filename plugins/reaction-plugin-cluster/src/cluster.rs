use std::{
    collections::BTreeMap,
    net::{SocketAddrV4, SocketAddrV6},
    sync::Arc,
};

use iroh::{Endpoint, EndpointAddr, EndpointId, endpoint::Connection};
use reaction_plugin::Exec;
use tokio::sync::{mpsc, oneshot};

use crate::{ActionInit, StreamInit, connection::ConnectionInitializer};

pub const ALPN: [&[u8]; 1] = ["reaction_cluster_1".as_bytes()];

type ShutdownNotification = oneshot::Receiver<oneshot::Sender<()>>;

pub struct Cluster {
    endpoint: Arc<Endpoint>,
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
        let endpoint = Arc::new(endpoint);

        let (endpoint_addr_tx, connection_rx) =
            ConnectionInitializer::new(endpoint.clone(), stream.name.clone(), stream.nodes.len());

        for node in stream.nodes.values() {
            endpoint_addr_tx.send(node.clone()).await.unwrap();
        }

        let this = Self {
            // No connection for now
            connections: Default::default(),
            // Values passed as-is
            stream,
            actions,
            endpoint,
            shutdown,
            endpoint_addr_tx,
            connection_rx,
        };
        tokio::spawn(async move { this.task().await });

        Ok(())
    }

    async fn task(mut self) {
        let action_rx = self.spawn_actions();

        // Ok donc là il faut :
        // - Que je réessaie plus tard les connections qui ont raté
        // - Que j'accepte des nouvelles connections
        // - Que j'ai une queue par noeud
        // - Que chaque élément de la queue puisse timeout
        // - Que j'envoie les messages de mes actions dans toutes les queues
        //
        // - Que j'écoute les messages de mes pairs et que je les renvoie à mon stream
        //
        // Et que je gère l'authentification en début de connection
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
