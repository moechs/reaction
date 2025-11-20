use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, Local, TimeDelta, Utc};
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::{Connection, VarInt},
};
use reaction_plugin::{Line, shutdown::ShutdownController};
use remoc::{Connect, rch::base};
use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, time::sleep};

use crate::cluster::{ALPN, UtcLine};

const START_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TIMEOUT: Duration = Duration::from_secs(60 * 60); // 1 hour
const TIMEOUT_FACTOR: f64 = 1.5;

const PROTOCOL_VERSION: u32 = 1;

enum Event {
    Tick,
    LocalMessageReceived(Option<UtcLine>),
    RemoteMessageReceived(Result<Option<RemoteMessage>, remoc::rch::base::RecvError>),
    ConnectionReceived(Option<Connection>),
}

enum Insert {
    Front,
    Back,
}

struct OwnConnection {
    connection: Connection,
    tx: base::Sender<RemoteMessage>,
    rx: base::Receiver<RemoteMessage>,
}

/// Handle a remote node.
/// Manage reception and sending of messages to this node.
/// Retry failed connections.
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
    delta: Option<Duration>,
    /// Next instant we'll try to connect
    next_try_connect: Option<Instant>,

    /// Max duration before we drop pending messages to a node we can't connect to.
    message_timeout: TimeDelta,
    /// Message we receive from actions
    message_rx: mpsc::Receiver<UtcLine>,
    /// Our queue of messages to send
    message_queue: VecDeque<UtcLine>,

    /// Messages we send from remote nodes to our own stream
    own_cluster_tx: remoc::rch::mpsc::Sender<Line>,

    /// shutdown
    shutdown: ShutdownController,
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
        shutdown: ShutdownController,
    ) -> Self {
        Self {
            cluster_name,
            remote,
            endpoint,
            connection: None,
            delta: None,
            next_try_connect: None,
            connection_rx,
            message_timeout,
            message_rx,
            message_queue: VecDeque::default(),
            own_cluster_tx,
            shutdown,
        }
    }

    /// Main loop
    pub async fn task(mut self) {
        self.try_connect().await;

        loop {
            let tick = sleep(if self.connection.is_none() {
                self.delta.unwrap_or(START_TIMEOUT)
            } else {
                // Still tick when we have a connection
                Duration::from_secs(60)
            });
            tokio::pin!(tick);

            let have_connection = self.connection.is_some();
            let maybe_conn_rx = self.connection.as_mut().map(|conn| conn.rx.recv());

            let event = tokio::select! {
                // Tick when we don't have a connection
                _ = tick, if !have_connection => Some(Event::Tick),
                // Receive remote message when we have a connection
                msg = maybe_conn_rx.unwrap(), if have_connection => Some(Event::RemoteMessageReceived(msg)),
                // Receive a connection from EndpointManager
                conn = self.connection_rx.recv() => Some(Event::ConnectionReceived(conn)),
                // Receive a message from local Actions
                msg = self.message_rx.recv() => Some(Event::LocalMessageReceived(msg)),
                // Quitting
                _ = self.shutdown.wait() => None,
            };

            match event {
                Some(event) => {
                    self.handle_event(event).await;
                    self.send_queue_messages().await;
                    self.drop_timeout_messages().await;
                }
                None => break,
            }
        }
    }

    async fn handle_event(&mut self, event: Event) {
        match event {
            Event::Tick => {
                self.handle_tick().await;
            }
            Event::ConnectionReceived(connection) => {
                self.handle_connection(connection).await;
            }
            Event::LocalMessageReceived(utc_line) => {
                self.handle_local_message(utc_line, Insert::Back).await;
            }
            Event::RemoteMessageReceived(message) => {
                self.handle_remote_message(message).await;
            }
        }
    }

    /// Try connecting to a remote endpoint
    /// Returns true if we have a valid connection now
    async fn try_connect(&mut self) -> bool {
        if self.connection.is_none() {
            match self.endpoint.connect(self.remote.clone(), ALPN[0]).await {
                Ok(connection) => self.handle_connection(Some(connection)).await,
                Err(err) => {
                    self.try_connect_error(err.to_string());
                    false
                }
            }
        } else {
            true
        }
    }

    async fn handle_tick(&mut self) {
        if self
            .next_try_connect
            .is_some_and(|next| next > Instant::now())
        {
            self.try_connect().await;
        }
    }

    async fn send_queue_messages(&mut self) {
        while self.connection.is_some()
            && let Some(message) = self.message_queue.pop_front()
        {
            self.handle_local_message(Some(message), Insert::Front)
                .await;
        }
    }

    async fn drop_timeout_messages(&mut self) {
        let now = Utc::now();
        let mut count = 0;
        while self
            .message_queue
            .front()
            .is_some_and(|element| element.1 + self.message_timeout < now)
        {
            self.message_queue.pop_front();
            count += 1;
        }
        if count > 0 {
            eprintln!(
                "DEBUG cluster {}: node {}: dropping {count} messages that reached timeout",
                self.cluster_name, self.remote.id,
            )
        }
    }

    /// Bootstrap a new Connection
    /// Returns true if we have a valid connection now
    async fn handle_connection(&mut self, connection: Option<Connection>) -> bool {
        match connection {
            None => {
                eprintln!(
                    "DEBUG cluster {}: ConnectionManager {}: quitting because EndpointManager has quit",
                    self.cluster_name, self.remote.id
                );
                self.quit();
                false
            }
            Some(connection) => {
                if self.connection.is_none() {
                    self.delta = None;
                    self.next_try_connect = None;

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
                } else {
                    eprintln!(
                        "WARN cluster {}: ignoring incoming connection from {}, as we already have a valid connection with it",
                        self.cluster_name, self.remote.id
                    );
                    true
                }
            }
        }
    }

    /// Update the state and log an error when bootstraping a new Connection
    fn try_connect_error(&mut self, err: String) {
        let delta = next_delta(self.delta);
        self.next_try_connect = Some(Instant::now() + delta);
        self.delta = Some(delta);
        eprintln!(
            "ERROR cluster {}: trying to connect to node {}: {err}",
            self.cluster_name, self.remote.id
        );
        eprintln!(
            "INFO cluster {}: retry connecting to node {} in {:?}",
            self.cluster_name, self.remote.id, self.delta
        );
    }

    async fn handle_remote_message(
        &mut self,
        message: Result<Option<RemoteMessage>, remoc::rch::base::RecvError>,
    ) {
        match message {
            Err(err) => {
                eprintln!(
                    "WARN cluster {}: error receiving message from node {}: {err}",
                    self.cluster_name, self.remote.id
                );
                self.close_connection(1, b"error receiving from your stream")
                    .await;
            }
            Ok(None) => {
                eprintln!(
                    "WARN cluster {}: node {} closed its stream",
                    self.cluster_name, self.remote.id
                );
                self.close_connection(1, b"you closed your stream").await;
            }
            Ok(Some(RemoteMessage::Version(_))) => {
                eprintln!(
                    "WARN cluster {}: node {} sent invalid message, ignoring",
                    self.cluster_name, self.remote.id
                );
            }
            Ok(Some(RemoteMessage::Quitting)) => {
                eprintln!(
                    "INFO cluster {}: node {} is quitting, bye bye",
                    self.cluster_name, self.remote.id
                );
                self.close_connection(0, b"you said you'll quit so I quit")
                    .await;
            }
            Ok(Some(RemoteMessage::Line(line))) => {
                let local_time = line.1.with_timezone(&Local);
                if let Err(err) = self.own_cluster_tx.send((line.0.clone(), local_time)).await {
                    eprintln!(
                        "ERROR cluster {}: could not send message to reaction stream: {err}",
                        self.cluster_name
                    );
                    eprintln!(
                        "INFO cluster {}: line that can't be sent: {}",
                        self.cluster_name, line.0
                    );
                    self.quit();
                }
            }
        }
    }

    async fn handle_local_message(&mut self, message: Option<UtcLine>, insert: Insert) {
        match message {
            None => {
                eprintln!(
                    "INFO cluster {}: no action remaining, quitting",
                    self.cluster_name
                );
                self.quit();
            }
            Some(message) => match &mut self.connection {
                Some(connection) => {
                    if let Err(err) = connection
                        .tx
                        .send(RemoteMessage::Line((
                            message.0.clone(),
                            message.1.with_timezone(&Utc),
                        )))
                        .await
                    {
                        eprintln!(
                            "INFO cluster {}: connection with node {} failed: {err}",
                            self.cluster_name, self.remote.id
                        );
                        self.message_queue.push_back(message);
                        self.close_connection(
                            0,
                            b"could not send a message to your channel so I quit",
                        )
                        .await;
                    }
                }
                None => {
                    if let Insert::Front = insert {
                        self.message_queue.push_front(message);
                    } else {
                        self.message_queue.push_back(message);
                    }
                }
            },
        }
    }

    async fn close_connection(&mut self, code: u32, reason: &[u8]) {
        if let Some(mut connection) = self.connection.take() {
            connection.rx.close().await;
            connection.connection.close(VarInt::from_u32(code), reason);
        }
    }

    fn quit(&mut self) {
        self.shutdown.ask_shutdown();
    }
}

/// Compute the next wait Duration.
/// We're multiplying the Duration by [`TIMEOUT_FACTOR`] and cap it to [`MAX_TIMEOUT`].
fn next_delta(delta: Option<Duration>) -> Duration {
    match delta {
        None => START_TIMEOUT,
        Some(delta) => {
            // Multiply timeout by TIMEOUT_FACTOR
            let delta = Duration::from_millis(((delta.as_millis() as f64) * TIMEOUT_FACTOR) as u64);
            // Cap to MAX_TIMEOUT
            if delta > MAX_TIMEOUT {
                MAX_TIMEOUT
            } else {
                delta
            }
        }
    }
}

/// All possible communication messages
/// Set as an enum for forward compatibility
#[derive(Serialize, Deserialize)]
pub enum RemoteMessage {
    /// Must be the first message sent over, then should not be sent again
    Version(u32),
    /// A line to transmit to your stream
    Line((String, DateTime<Utc>)),
    /// Announce the node is closing
    Quitting,
}

/// Open accept one stream and create one stream.
/// This way, there is no need to know if we created or accepted the connection.
async fn open_channels(
    connection: &Connection,
) -> Result<(base::Sender<RemoteMessage>, base::Receiver<RemoteMessage>), String> {
    let output = connection.open_uni().await.map_err(|err| err.to_string())?;

    let input = connection
        .accept_uni()
        .await
        .map_err(|err| err.to_string())?;

    let (conn, mut tx, mut rx) = Connect::io_buffered(remoc::Cfg::default(), input, output, 1024)
        .await
        .map_err(|err| err.to_string())?;

    tokio::spawn(conn);

    tx.send(RemoteMessage::Version(PROTOCOL_VERSION))
        .await
        .map_err(|err| err.to_string())?;

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
