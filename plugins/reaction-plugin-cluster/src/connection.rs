use std::{collections::VecDeque, sync::Arc, time::Duration};

use chrono::TimeDelta;
use iroh::{Endpoint, EndpointAddr, endpoint::Connection};
use reaction_plugin::Line;
use remoc::{Connect, rch::base};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::mpsc,
    time::{Sleep, sleep},
};

use crate::cluster::{ALPN, UtcLine};

const START_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TIMEOUT: Duration = Duration::from_secs(60 * 60); // 1 hour
const TIMEOUT_FACTOR: f64 = 1.5;

const PROTOCOL_VERSION: u32 = 1;

enum Event {
    Tick,
    LocalMessageReceived(UtcLine),
    RemoteMessageReceived(RemoteMessage),
    ConnectionReceived(Connection),
}

struct OwnConnection {
    connection: Connection,
    tx: base::Sender<RemoteMessage>,
    rx: base::Receiver<RemoteMessage>,
}

pub struct ConnectionManager {
    /// Cluster's name (for logging)
    cluster_name: String,

    /// The remote node we're communicating with
    remote: EndpointAddr,
    /// Endpoint
    endpoint: Arc<Endpoint>,

    /// The EndpointManager sending us a connection (whether we asked for it or not)
    connection_rx: mpsc::Receiver<Connection>,
    /// Our own connection (when we have one)
    connection: Option<OwnConnection>,

    /// Delta we'll use next time we'll try to connect to remote
    delta: Duration,
    /// When this Future resolves, we'll retry connecting to remote
    tick: Option<Sleep>,

    /// Max duration before we drop pending messages to a node we can't connect to.
    message_timeout: TimeDelta,
    /// Message we receive from actions
    message_rx: mpsc::Receiver<UtcLine>,
    /// Our queue of messages to send
    message_queue: VecDeque<UtcLine>,

    /// Messages we send from remote nodes to our own stream
    own_cluster_tx: remoc::rch::mpsc::Sender<Line>,
}

impl ConnectionManager {
    pub fn new(
        cluster_name: String,
        remote: EndpointAddr,
        endpoint: Arc<Endpoint>,
        connection_rx: mpsc::Receiver<Connection>,
        message_timeout: TimeDelta,
        message_rx: mpsc::Receiver<UtcLine>,
        own_cluster_tx: remoc::rch::mpsc::Sender<Line>,
    ) -> Self {
        Self {
            cluster_name,
            remote,
            endpoint,
            connection: None,
            delta: Duration::default(),
            tick: None,
            connection_rx,
            message_timeout,
            message_rx,
            message_queue: VecDeque::default(),
            own_cluster_tx,
        }
    }

    pub async fn task(mut self) {
        self.try_connect().await;

        loop {
            // TODO event
            let event = Event::Tick;

            self.handle_event(event).await;
        }
    }

    /// Main loop
    async fn handle_event(&mut self, event: Event) {
        match event {
            Event::Tick => {
                // TODO
                self.try_connect().await;
            }
            Event::ConnectionReceived(connection) => {
                // TODO
            }
            Event::LocalMessageReceived(utc_line) => {
                // TODO
            }
            Event::RemoteMessageReceived(remote_message) => {
                // TODO
            }
        }
    }

    /// Try connecting to a remote endpoint
    /// Returns true if we have a valid connection now
    async fn try_connect(&mut self) -> bool {
        if self.connection.is_none() {
            match self.endpoint.connect(self.remote.clone(), ALPN[0]).await {
                Ok(connection) => self.handle_connection(connection).await,
                Err(err) => {
                    self.try_connect_error(err.to_string());
                    false
                }
            }
        } else {
            true
        }
    }

    /// Bootstrap a new Connection
    /// Returns true if we have a valid connection now
    async fn handle_connection(&mut self, connection: Connection) -> bool {
        self.delta = Duration::default();
        self.tick = None;

        match open_channels(&connection).await {
            Ok((tx, rx)) => {
                self.connection = Some(OwnConnection { connection, tx, rx });
                true
            }
            Err(err) => {
                self.try_connect_error(err);
                false
            }
        }
    }

    /// Update the state and log an error when bootstraping a new Connection
    async fn try_connect_error(&mut self, err: String) {
        self.delta = next_delta(self.delta);
        self.tick = Some(sleep(self.delta));
        eprintln!(
            "ERROR cluster {}: node {}: {err}",
            self.cluster_name, self.remote.id
        );
        eprintln!(
            "INFO cluster {}: retry connecting to node {} in {:?}",
            self.cluster_name, self.remote.id, self.delta
        );
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

/// All possible communication messages
/// Set as an enum for forward compatibility
#[derive(Serialize, Deserialize)]
pub enum RemoteMessage {
    /// Must be the first message sent over, then should not be sent again
    Version(u32),
    /// A line to transmit to your stream
    Line(UtcLine),
    /// Announce the node is closing
    Quitting,
}

/// Open accept one stream and create one stream.
/// This way, there is no need to know if we created or accepted the connection.
async fn open_channels(
    connection: &Connection,
) -> Result<(base::Sender<RemoteMessage>, base::Receiver<RemoteMessage>), String> {
    let output = connection
        .open_uni()
        .await
        .map_err(|err| format!("{err}"))?;

    let input = connection
        .accept_uni()
        .await
        .map_err(|err| format!("{err}"))?;

    let (conn, mut tx, mut rx) = Connect::io_buffered(remoc::Cfg::default(), input, output, 1024)
        .await
        .map_err(|err| format!("{err}"))?;

    tokio::spawn(conn);

    tx.send(RemoteMessage::Version(PROTOCOL_VERSION)).await;

    match rx.recv().await {
        // Good protocol version!
        Ok(Some(RemoteMessage::Version(PROTOCOL_VERSION))) => Ok((tx, rx)),
        // Errors
        Ok(Some(RemoteMessage::Version(other))) => Err(format!(
            "incompatible version: {other}. We use {PROTOCOL_VERSION}. Consider upgrading the node with the older version."
        )),
        Ok(Some(RemoteMessage::Line(_))) => Err(format!(
            "incorrect protocol message: remote did not send its protocol version."
        )),
        Ok(Some(RemoteMessage::Quitting)) => Err("remote unexpectedly quit".into()),
        Ok(None) => Err("remote unexpectedly closed its channel".into()),
        Err(err) => Err(format!("could not receive message: {err}")),
    }
}
