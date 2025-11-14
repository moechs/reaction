use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use iroh::endpoint::Incoming;
use iroh::{Endpoint, PublicKey};
use iroh::{EndpointAddr, endpoint::Connection};
use tokio::{
    sync::mpsc,
    time::{Instant, sleep, sleep_until},
};

use crate::cluster::ALPN;

const START_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TIMEOUT: Duration = Duration::from_secs(60 * 60); // 1 hour
const TIMEOUT_FACTOR: f64 = 1.5;

enum Event {
    TryConnect(EndpointAddr),
    Quit,
    Tick,
    Incoming(Option<Incoming>),
}

enum Break {
    Yes,
    No,
}

pub struct EndpointManager {
    /// The [`iroh::Endpoint`] to manage
    endpoint: Endpoint,
    /// Cluster's name (for logging)
    cluster_name: String,
    /// Map of remote Endpoints to try to connect to
    retry_connections: BTreeMap<Instant, (EndpointAddr, Duration)>,
    /// Set of PublicKeys we're trying to connect to
    all_connections: BTreeMap<PublicKey, Instant>,
    /// Connection requests from the [`crate::Cluster`]
    endpoint_addr_rx: mpsc::Receiver<EndpointAddr>,
    /// Connection sender to the [`crate::Cluster`]
    connection_tx: mpsc::Sender<Connection>,
}

impl EndpointManager {
    pub fn new(
        endpoint: Endpoint,
        cluster_name: String,
        cluster_size: usize,
    ) -> (mpsc::Sender<EndpointAddr>, mpsc::Receiver<Connection>) {
        let (tx1, rx1) = mpsc::channel(cluster_size);
        let (tx2, rx2) = mpsc::channel(cluster_size);

        tokio::spawn(async move {
            Self {
                endpoint,
                cluster_name,
                retry_connections: Default::default(),
                all_connections: Default::default(),
                endpoint_addr_rx: rx1,
                connection_tx: tx2,
            }
            .task()
            .await
        });

        (tx1, rx2)
    }

    async fn task(&mut self) {
        let mut tick = sleep(Duration::default());

        loop {
            // Uncomment this line and comment the select! for faster development in this function
            // let event = Event::TryConnect(self.endpoint_addr_rx.recv().await);
            let event = tokio::select! {
                received = self.endpoint_addr_rx.recv() => {
                    match received {
                        Some(endpoint_addr) => Event::TryConnect(endpoint_addr),
                        None => Event::Quit,
                    }
                }
                incoming = self.endpoint.accept() => Event::Incoming(incoming),
                _ = tick => Event::Tick,
            };

            if let Break::Yes = self.handle_event(event).await {
                break;
            }

            // Tick at next deadline
            tick = sleep_until(
                self.retry_connections
                    .keys()
                    .next()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| Instant::now() + MAX_TIMEOUT),
            );
        }

        self.endpoint.close().await
    }

    async fn handle_event(&mut self, event: Event) -> Break {
        match event {
            Event::Quit => return Break::Yes,

            Event::TryConnect(endpoint_addr) => match self.try_connect(endpoint_addr).await {
                Ok(connection) => return self.check_and_send_connection(connection).await,
                Err(endpoint_addr) => {
                    self.insert_endpoint(endpoint_addr, START_TIMEOUT);
                }
            },

            Event::Tick => {
                if let Some((endpoint_addr, delta)) = self.pop_next_endpoint() {
                    match self.try_connect(endpoint_addr).await {
                        Ok(connection) => {
                            return self.check_and_send_connection(connection).await;
                        }
                        Err(endpoint_addr) => {
                            let delta = next_delta(delta);
                            self.insert_endpoint(endpoint_addr, delta);
                        }
                    }
                }
            }

            Event::Incoming(incoming) => {
                // FIXME a malicious actor could maybe prevent a node from connecting to
                // its cluster by sending lots of invalid slow connection requests?
                // We could lower its priority https://docs.rs/tokio/latest/tokio/macro.select.html#fairness
                // And/or moving the handshake to another task
                if let Some(incoming) = incoming {
                    let remote_address = incoming.remote_address();
                    let remote_address_validated = incoming.remote_address_validated();
                    match incoming.await {
                        Ok(connection) => {
                            return self.check_and_send_connection(connection).await;
                        }
                        Err(err) => {
                            if remote_address_validated {
                                eprintln!("INFO refused connection from {}: {err}", remote_address)
                            } else {
                                eprintln!("INFO refused connection: {err}")
                            }
                        }
                    }
                }
            }
        }
        Break::No
    }

    /// Schedule an endpoint to try to connect to later
    fn insert_endpoint(&mut self, endpoint_addr: EndpointAddr, delta: Duration) {
        if !delta.is_zero() {
            eprintln!(
                "INFO cluster {}: retry connecting to node {} in {:?}",
                self.cluster_name, endpoint_addr.id, delta
            );
        }
        let next = Instant::now() + delta;
        // Schedule this address for later
        self.all_connections.insert(endpoint_addr.id, next);
        self.retry_connections.insert(next, (endpoint_addr, delta));
    }

    /// Returns the next endpoint we should try to connect to
    fn pop_next_endpoint(&mut self) -> Option<(EndpointAddr, Duration)> {
        if self
            .retry_connections
            .keys()
            .next()
            .is_some_and(|time| time < &Instant::now())
        {
            let (_, tuple) = self.retry_connections.pop_first().unwrap();
            self.all_connections.remove(&tuple.0.id);
            Some(tuple)
        } else {
            None
        }
    }

    /// Try connecting to a remote endpoint
    async fn try_connect(&self, addr: EndpointAddr) -> Result<Connection, EndpointAddr> {
        match self.endpoint.connect(addr.clone(), ALPN[0]).await {
            Ok(connection) => Ok(connection),
            Err(err) => {
                eprintln!(
                    "ERROR cluster {}: node {}: {err}",
                    self.cluster_name, addr.id
                );
                Err(addr)
            }
        }
    }

    /// Check that an incoming connection is an endpoint we're trying to connect,
    /// and send it to the [`Cluster`]
    async fn check_and_send_connection(&mut self, connection: Connection) -> Break {
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

        match self.all_connections.remove(&remote_id) {
            None => {
                eprintln!(
                    "WARN cluster {}: new peer's id '{remote_id}' is not in our list, refusing incoming connection.",
                    self.cluster_name
                );
                eprintln!(
                    "INFO cluster {}: {}, {}",
                    self.cluster_name,
                    "maybe we're already connected to it, maybe it's not from our cluster",
                    "maybe it is new and it has not been configured yet on this node"
                );
                return Break::No;
            }
            Some(time) => {
                self.retry_connections.remove(&time);
            }
        }

        // TODO persist the incoming address, so that we don't forget this address

        if let Err(_) = self.connection_tx.send(connection).await {
            // This means the main cluster loop has exited, so let's quit
            return Break::Yes;
        }
        Break::No
    }
}

/// Compute the next wait Duration.
/// We're multiplying the Duration by [`TIMEOUT_FACTOR`] and cap it to [`MAX_TIMEOUT`].
fn next_delta(delta: Duration) -> Duration {
    // Multiply timeout by TIMEOUT_FACTOR
    let delta = Duration::from_millis(((delta.as_millis() as f64) * TIMEOUT_FACTOR) as u64);
    // Cap to MAX_TIMEOUT
    if delta > MAX_TIMEOUT {
        MAX_TIMEOUT
    } else {
        delta
    }
}
