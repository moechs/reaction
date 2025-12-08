use std::collections::BTreeMap;
use std::sync::Arc;

use iroh::{Endpoint, PublicKey, endpoint::Incoming};
use reaction_plugin::shutdown::ShutdownController;
use tokio::sync::mpsc;

use crate::{connection::ConnOrConn, key::Show};

enum Break {
    Yes,
    No,
}

pub struct EndpointManager {
    /// The [`iroh::Endpoint`] to manage
    endpoint: Arc<Endpoint>,
    /// Cluster's name (for logging)
    cluster_name: String,
    /// Connection sender to the Connection Managers
    connections_tx: BTreeMap<PublicKey, mpsc::Sender<ConnOrConn>>,
    /// shutdown
    shutdown: ShutdownController,
}

impl EndpointManager {
    pub fn new(
        endpoint: Arc<Endpoint>,
        cluster_name: String,
        connections_tx: BTreeMap<PublicKey, mpsc::Sender<ConnOrConn>>,
        shutdown: ShutdownController,
    ) {
        tokio::spawn(async move {
            Self {
                endpoint,
                cluster_name,
                connections_tx,
                shutdown,
            }
            .task()
            .await
        });
    }

    async fn task(&mut self) {
        loop {
            // Uncomment this line and comment the select! for faster development in this function
            // let event = Event::TryConnect(self.endpoint_addr_rx.recv().await);
            let incoming = tokio::select! {
                incoming = self.endpoint.accept() => incoming,
                _ = self.shutdown.wait() => None,
            };

            match incoming {
                Some(incoming) => {
                    if let Break::Yes = self.handle_incoming(incoming).await {
                        break;
                    }
                }
                None => break,
            }
        }

        self.endpoint.close().await
    }

    async fn handle_incoming(&mut self, incoming: Incoming) -> Break {
        eprintln!(
            "DEBUG cluster {}: EndpointManager: receiving connection",
            self.cluster_name,
        );
        // FIXME a malicious actor could maybe prevent a node from connecting to
        // its cluster by sending lots of invalid slow connection requests?
        // This function could be moved to a new 'oneshot' task instead
        let remote_address = incoming.remote_address();
        let remote_address_validated = incoming.remote_address_validated();
        let connection = match incoming.await {
            Ok(connection) => connection,
            Err(err) => {
                if remote_address_validated {
                    eprintln!("INFO refused connection from {}: {err}", remote_address)
                } else {
                    eprintln!("INFO refused connection: {err}")
                }
                return Break::No;
            }
        };

        let remote_id = match connection.remote_id() {
            Ok(id) => id,
            Err(err) => {
                eprintln!(
                    "ERROR cluster {}: could not retrieve peer's id: {err}",
                    self.cluster_name
                );
                return Break::No;
            }
        };

        match self.connections_tx.get(&remote_id) {
            None => {
                eprintln!(
                    "WARN cluster {}: incoming connection from node '{}', ip: {} is not in our list, refusing incoming connection.",
                    self.cluster_name,
                    remote_id.show(),
                    remote_address
                );
                eprintln!(
                    "INFO cluster {}: {}, {}",
                    self.cluster_name,
                    "maybe it's not from our cluster,",
                    "maybe this node's configuration has not yet been updated to add this new node."
                );
                return Break::No;
            }
            Some(tx) => {
                if let Err(_) = tx.send(ConnOrConn::Connection(connection)).await {
                    eprintln!(
                        "DEBUG cluster {}: EndpointManager: quitting because ConnectionManager has quit",
                        self.cluster_name,
                    );
                    self.shutdown.ask_shutdown();
                    return Break::Yes;
                }
                eprintln!(
                    "DEBUG cluster {}: EndpointManager: receiving connection from {}",
                    self.cluster_name,
                    remote_id.show(),
                );
            }
        }

        // TODO persist the incoming address, so that we don't forget this address

        Break::No
    }
}
