use std::collections::BTreeMap;
use std::time::Duration;

use iroh::Endpoint;
use iroh::endpoint::Incoming;
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
            tick = sleep_until(*self.retry_connections.keys().next().unwrap());
        }

        self.endpoint.close().await
    }

    async fn handle_event(&mut self, event: Event) -> Break {
        match event {
            Event::Quit => return Break::Yes,

            Event::TryConnect(endpoint_addr) => match self.try_connect(endpoint_addr).await {
                Ok(connection) => return self.handshake(connection, true).await,
                Err(endpoint_addr) => {
                    self.insert_address(endpoint_addr, START_TIMEOUT);
                }
            },

            Event::Tick => {
                if self
                    .retry_connections
                    .keys()
                    .next()
                    .is_some_and(|time| time < &Instant::now())
                {
                    let (_, (endpoint_addr, delta)) = self.retry_connections.pop_first().unwrap();
                    match self.try_connect(endpoint_addr).await {
                        Ok(connection) => return self.handshake(connection, true).await,
                        Err(endpoint_addr) => {
                            let delta = next_delta(delta);
                            self.insert_address(endpoint_addr, delta);
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
                        Ok(connection) => return self.handshake(connection, false).await,
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

    fn insert_address(&mut self, endpoint_addr: EndpointAddr, delta: Duration) {
        if !delta.is_zero() {
            eprintln!(
                "INFO cluster {}: retry connecting to node {} in {:?}",
                self.cluster_name, endpoint_addr.id, delta
            );
        }
        let now = Instant::now();
        // Schedule this address for later
        self.retry_connections
            .insert(now + delta, (endpoint_addr, delta));
    }

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

    async fn handshake(&mut self, connection: Connection, initiated_by_me: bool) -> Break {
        // TODO validate handshake before sending
        if let Err(_) = self.connection_tx.send(connection).await {
            // This means the main cluster loop has exited, so let's quit
            return Break::Yes;
        }
        Break::No
    }
}

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
