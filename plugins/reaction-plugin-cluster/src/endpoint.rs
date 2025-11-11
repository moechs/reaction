use std::arch::x86_64::_mm_sha256rnds2_epu32;
use std::collections::BTreeMap;
use std::time::Duration;

use iroh::Endpoint;
use iroh::endpoint::{ConnectionStats, Incoming};
use iroh::{EndpointAddr, endpoint::Connection};
use rand::{Rng, rng};
use sha2::{Digest, Sha256};
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
    shared_secret: String,
}

impl EndpointManager {
    pub fn new(
        endpoint: Endpoint,
        cluster_name: String,
        shared_secret: String,
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
                shared_secret,
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
        let remote_id = connection
            .remote_id()
            .map(|id| id.to_string())
            .unwrap_or("".to_string());

        // Send challenge and check response
        let ask_future = {
            let connection = connection.clone();
            let shared_secret = self.shared_secret.clone();
            let remote_id = remote_id.clone();

            async move {
                let (mut tx, mut rx) = connection.open_bi().await.map_err(|err| {
                    format!(
                        "could not init a QUIC communication channel with node {remote_id}: {err}",
                    )
                })?;

                let salt: [u8; 32] = rand::rng().random();

                tx.write_all(&salt)
                    .await
                    .map_err(|err| format!("could not send data to node {remote_id}: {err}"))?;

                let mut concat: Vec<u8> = Vec::with_capacity(64);
                concat.extend_from_slice(shared_secret.as_bytes());
                concat.extend_from_slice(&salt);
                let expected_hash = Sha256::digest(concat);

                let mut received_hash = [0u8; 32];
                rx.read_exact(&mut received_hash)
                    .await
                    .map_err(|err| format!("could not read data from node {remote_id}: {err}"))?;

                if expected_hash.iter().eq(received_hash.iter()) {
                    // Send "ok"
                    // Return Ok(())
                    tx.write_all(&[1u8])
                        .await
                        .map_err(|err| format!("could not send data to node {remote_id}: {err}"))
                } else {
                    let _ = tx.write_all(&[0u8]).await;
                    Err(format!(
                        "hash mismatch from node {remote_id}. It most probably has a different `secret_key` set."
                    ))
                }
            }
        };

        // Receive challenge and send response
        let answer_future = {
            let connection = connection.clone();
            let shared_secret = self.shared_secret.clone();
            let remote_id = remote_id.clone();

            async move {
                let (mut tx, mut rx) = connection.accept_bi().await.map_err(|err| {
                    format!(
                        "could not receive a QUIC communication channel from node {remote_id}: {err}",
                    )
                })?;

                let mut received_salt = [0u8; 32];
                rx.read_exact(&mut received_salt)
                    .await
                    .map_err(|err| format!("could not read data from node {remote_id}: {err}"))?;

                let mut concat: Vec<u8> = Vec::with_capacity(64);
                concat.extend_from_slice(shared_secret.as_bytes());
                concat.extend_from_slice(&received_salt);
                let computed_hash = Sha256::digest(concat);

                tx.write_all(&computed_hash)
                    .await
                    .map_err(|err| format!("could not send data to node {remote_id}: {err}"))?;

                let mut response = [0u8; 1];
                rx.read_exact(&mut response)
                    .await
                    .map_err(|err| format!("could not read data from node {remote_id}: {err}"))?;

                if response[0] == 1 {
                    Ok(())
                } else {
                    Err(format!(
                        "node {remote_id} announced a hash mismatch. It most probably has a different `secret_key` set."
                    ))
                }
            }
        };

        let (ask_result, answer_result) = tokio::join!(ask_future, answer_future);
        if let Err(err) = &ask_result {
            eprintln!("ERROR {err}");
        }
        if let Err(err) = &answer_result {
            eprintln!("ERROR {err}");
        }

        if ask_result.is_ok() && answer_result.is_ok() {
            if let Err(_) = self.connection_tx.send(connection).await {
                // This means the main cluster loop has exited, so let's quit
                return Break::Yes;
            }
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
