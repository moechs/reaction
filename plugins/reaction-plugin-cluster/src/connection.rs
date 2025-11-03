use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use iroh::Endpoint;
use iroh::{EndpointAddr, endpoint::Connection};
use tokio::{
    sync::mpsc,
    time::{Instant, sleep, sleep_until},
};

use crate::cluster::ALPN;

const START_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TIMEOUT: Duration = Duration::from_secs(60 * 60); // 1 hour
const TIMEOUT_FACTOR: f64 = 1.5;

pub struct ConnectionInitializer {
    endpoint: Arc<Endpoint>,
    cluster_name: String,
    retry_connections: BTreeMap<Instant, (EndpointAddr, Duration)>,
    endpoint_addr_rx: mpsc::Receiver<EndpointAddr>,
    connection_tx: mpsc::Sender<Connection>,
}

impl ConnectionInitializer {
    pub fn new(
        endpoint: Arc<Endpoint>,
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
            // Uncomment this line and comment the select for faster development in this function
            // let option = Some(self.endpoint_addr_rx.recv().await);
            let option = tokio::select! {
                endpoint_addr = self.endpoint_addr_rx.recv() => Some(endpoint_addr),
                _ = tick => None,
            };
            if let Some(option) = option {
                if let Some(endpoint_addr) = option {
                    match self.try_connect(endpoint_addr).await {
                        Ok(connection) => {
                            if let Err(_) = self.connection_tx.send(connection).await {
                                // This means the main cluster loop has quit, so let's quit
                                break;
                            }
                        }
                        Err(endpoint_addr) => {
                            self.insert_address(endpoint_addr, START_TIMEOUT);
                        }
                    }
                } else {
                    break;
                }
            } else {
                if self
                    .retry_connections
                    .keys()
                    .next()
                    .is_some_and(|time| time < &Instant::now())
                {
                    let (_, (endpoint_addr, delta)) = self.retry_connections.pop_first().unwrap();
                    match self.try_connect(endpoint_addr).await {
                        Ok(connection) => {
                            if let Err(_) = self.connection_tx.send(connection).await {
                                // This means the main cluster loop has quit, so let's quit
                                break;
                            }
                        }
                        Err(endpoint_addr) => {
                            // Multiply timeout by TIMEOUT_FACTOR
                            let delta = Duration::from_millis(
                                ((delta.as_millis() as f64) * TIMEOUT_FACTOR) as u64,
                            );
                            // Cap to MAX_TIMEOUT
                            let delta = if delta > MAX_TIMEOUT {
                                MAX_TIMEOUT
                            } else {
                                delta
                            };
                            self.insert_address(endpoint_addr, delta);
                        }
                    }
                }
            }
            // Tick at next deadline
            tick = sleep_until(*self.retry_connections.keys().next().unwrap());
        }
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
                    "ERROR cluster {}: connecting to node {}: {err}",
                    self.cluster_name, addr.id
                );
                Err(addr)
            }
        }
    }
}
