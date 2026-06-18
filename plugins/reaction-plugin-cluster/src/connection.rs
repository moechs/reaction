use std::{cmp::max, io::Error as IoError, sync::Arc, time::Duration};

use futures::FutureExt;
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::{Connection, RecvStream, SendStream, VarInt},
};
use rand::random_range;
use reaction_plugin::{Line, shutdown::ShutdownController};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
    sync::mpsc,
    time::sleep,
};
use treedb::{
    Database, Tree,
    helpers::{to_string, to_time},
    time::{Time, now},
};

use crate::{
    cluster::{ALPN, UtcLine},
    key::Show,
};

const PROTOCOL_VERSION: u32 = 1;

const CLOSE_RECV: (u32, &[u8]) = (1, b"error receiving from your stream");
const CLOSE_CLOSED: (u32, &[u8]) = (2, b"you closed your stream");
const CLOSE_SEND: (u32, &[u8]) = (3, b"could not send a message to your channel so I quit");

type MaybeRemoteLine = Result<Option<(String, Time)>, IoError>;

enum Event {
    LocalMessageReceived(Option<UtcLine>),
    RemoteMessageReceived(MaybeRemoteLine),
    ConnectionReceived(Option<ConnOrConn>),
}

pub struct OwnConnection {
    connection: Connection,
    id: u64,

    line_tx: BufWriter<SendStream>,
    line_rx: BufReader<RecvStream>,

    next_time_secs: Option<u64>,
    next_time_nanos: Option<u32>,
    next_len: Option<usize>,
    next_line: Option<Vec<u8>>,
}

impl OwnConnection {
    fn new(
        connection: Connection,
        id: u64,
        line_tx: BufWriter<SendStream>,
        line_rx: BufReader<RecvStream>,
    ) -> Self {
        Self {
            connection,
            id,
            line_tx,
            line_rx,
            next_time_secs: None,
            next_time_nanos: None,
            next_len: None,
            next_line: None,
        }
    }

    /// Send a line to peer.
    ///
    /// Time is a std::time::Duration since UNIX_EPOCH, which is defined as UTC
    /// So it's safe to use between nodes using different timezones
    async fn send_line(&mut self, line: &String, time: &Time) -> Result<(), std::io::Error> {
        self.line_tx.write_u64(time.as_secs()).await?;
        self.line_tx.write_u32(time.subsec_nanos()).await?;
        self.line_tx.write_u32(line.len() as u32).await?;
        self.line_tx.write_all(line.as_bytes()).await?;
        self.line_tx.flush().await?;
        Ok(())
    }

    /// Cancel-safe function that returns next line from peer
    /// Returns None if we don't have all data yet.
    async fn recv_line(&mut self) -> MaybeRemoteLine {
        if self.next_time_secs.is_none() {
            self.next_time_secs = Some(self.line_rx.read_u64().await?);
        }
        if self.next_time_nanos.is_none() {
            self.next_time_nanos = Some(self.line_rx.read_u32().await?);
        }
        if self.next_len.is_none() {
            self.next_len = Some(self.line_rx.read_u32().await? as usize);
        }
        // Ok we have next_len.is_some()
        let next_len = self.next_len.clone().unwrap();

        if self.next_line.is_none() {
            self.next_line = Some(Vec::with_capacity(next_len));
        }
        // Ok we have next_line.is_some()
        let next_line = self.next_line.as_mut().unwrap();

        let actual_len = next_line.len();
        // Resize to wanted length
        next_line.resize(next_len, 0);

        // Read bytes
        let bytes_read = self
            .line_rx
            .read(&mut next_line[actual_len..next_len])
            .await?;
        // Truncate possibly unread bytes
        next_line.truncate(actual_len + bytes_read);

        // Let's test if we read all bytes
        if next_line.len() == next_len {
            // Ok we have a full line
            self.next_len.take();
            let line = String::try_from(self.next_line.take().unwrap()).map_err(|err| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string())
            })?;
            let time = Time::new(
                self.next_time_secs.take().unwrap(),
                self.next_time_nanos.take().unwrap(),
            );
            Ok(Some((line, time)))
        } else {
            // Ok we don't have a full line, will be next time!
            Ok(None)
        }
    }
}

pub enum ConnOrConn {
    Connection(Connection),
    OwnConnection(OwnConnection),
}

/// Handle a remote node.
/// Manage reception and sending of messages to this node.
/// Retry failed connections.
pub struct ConnectionManager {
    /// Cluster's name (for logging)
    cluster_name: String,
    /// The remote node we're communicating with (for logging)
    node_id: String,
    /// Remote
    remote: EndpointAddr,
    /// Endpoint
    endpoint: Arc<Endpoint>,

    /// Cancel asking for a connection
    cancel_ask_connection: Option<mpsc::Sender<()>>,
    /// Create a delegated task to send ourselves a connection
    connection_tx: mpsc::Sender<ConnOrConn>,
    /// The EndpointManager or our delegated task sending us a connection (whether we asked for it or not)
    connection_rx: mpsc::Receiver<ConnOrConn>,
    /// Our own connection (when we have one)
    connection: Option<OwnConnection>,
    /// Last connexion ID, used to have a determinist way to choose between conflicting connections
    last_connection_id: u64,

    /// Max duration before we drop pending messages to a node we can't connect to.
    message_timeout: Duration,
    /// Message we receive from actions
    message_rx: mpsc::Receiver<UtcLine>,
    /// Our queue of messages to send
    message_queue: Tree<Time, Arc<String>>,

    /// Messages we send from remote nodes to our own stream
    own_cluster_tx: remoc::rch::mpsc::Sender<Line>,

    /// shutdown
    shutdown: ShutdownController,
}

impl ConnectionManager {
    pub async fn new(
        cluster_name: String,
        remote: EndpointAddr,
        endpoint: Arc<Endpoint>,
        message_timeout: Duration,
        message_rx: mpsc::Receiver<UtcLine>,
        own_cluster_tx: remoc::rch::mpsc::Sender<Line>,
        db: &mut Database,
        shutdown: ShutdownController,
    ) -> Result<(Self, mpsc::Sender<ConnOrConn>), String> {
        let node_id = remote.id.show();

        let message_queue = db
            .open_tree(
                format!("message_queue_{}_{}", endpoint.id().show(), node_id),
                message_timeout,
                |(key, value)| Ok((to_time(&key)?, Arc::new(to_string(&value)?))),
            )
            .await?;

        let (connection_tx, connection_rx) = mpsc::channel(1);
        Ok((
            Self {
                cluster_name,
                node_id,
                remote,
                endpoint,
                connection: None,
                cancel_ask_connection: None,
                connection_tx: connection_tx.clone(),
                connection_rx,
                last_connection_id: 0,
                message_timeout,
                message_rx,
                message_queue,
                own_cluster_tx,
                shutdown,
            },
            connection_tx,
        ))
    }

    /// Main loop
    pub async fn task(mut self) {
        self.ask_connection();
        loop {
            let have_connection = self.connection.is_some();
            let maybe_conn_rx = self
                .connection
                .as_mut()
                .map(|conn| conn.recv_line().boxed())
                // This Future will never be polled because of the if in select!
                // It still needs to be present because the branch will be evaluated
                // so we can't unwrap
                .unwrap_or(false_recv().boxed());

            let event = tokio::select! {
                biased;
                // Quitting
                _ = self.shutdown.wait() => None,
                // Receive a connection from EndpointManager
                conn = self.connection_rx.recv() => Some(Event::ConnectionReceived(conn)),
                // Receive remote message when we have a connection
                msg = maybe_conn_rx, if have_connection => Some(Event::RemoteMessageReceived(msg)),
                // Receive a message from local Actions
                msg = self.message_rx.recv() => Some(Event::LocalMessageReceived(msg)),
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
            Event::ConnectionReceived(connection) => {
                self.handle_connection(connection).await;
            }
            Event::LocalMessageReceived(utc_line) => {
                self.handle_local_message(utc_line).await;
            }
            Event::RemoteMessageReceived(message) => {
                self.handle_remote_message(message).await;
            }
        }
    }

    async fn send_queue_messages(&mut self) {
        while let Some(connection) = &mut self.connection
            && let Some((time, line)) = self
                .message_queue
                .first_key_value()
                .map(|(k, v)| (k.clone(), v.clone()))
        {
            if let Err(err) = connection.send_line(&line, &time).await {
                eprintln!(
                    "INFO cluster {}: connection with node {} failed: {err}",
                    self.cluster_name, self.node_id,
                );
                self.close_connection(CLOSE_SEND).await;
            } else {
                self.message_queue.remove(&time).await;
                eprintln!(
                    "DEBUG cluster {}: node {}: sent a local message to remote: {}",
                    self.cluster_name, self.node_id, line
                );
            }
        }
    }

    async fn drop_timeout_messages(&mut self) {
        let now = now();
        let mut count = 0;
        loop {
            // We have a next key and it reached timeout
            if let Some(next_key) = self.message_queue.first_key_value().map(|kv| kv.0.clone())
                && next_key + self.message_timeout < now
            {
                self.message_queue.remove(&next_key).await;
                count += 1;
            } else {
                break;
            }
        }
        if count > 0 {
            eprintln!(
                "DEBUG cluster {}: node {}: dropping {count} messages that reached timeout",
                self.cluster_name, self.node_id,
            )
        }
    }

    /// Bootstrap a new Connection
    /// Returns true if we have a valid connection now
    async fn handle_connection(&mut self, connection: Option<ConnOrConn>) {
        match connection {
            None => {
                eprintln!(
                    "DEBUG cluster {}: ConnectionManager {}: quitting because EndpointManager has quit",
                    self.cluster_name, self.node_id,
                );
                self.quit();
            }
            Some(connection) => {
                if let Some(cancel) = self.cancel_ask_connection.take() {
                    let _ = cancel.send(()).await;
                }

                let last_connection_id = self.last_connection_id;
                let mut insert_connection = |own_connection: OwnConnection| {
                    if self
                        .connection
                        .as_ref()
                        .is_none_or(|old_own| old_own.id < own_connection.id)
                    {
                        self.last_connection_id = own_connection.id;
                        self.connection = Some(own_connection);
                    } else {
                        eprintln!(
                            "WARN cluster {}: node {}: ignoring incoming connection, as we already have a valid connection with it and our connection id is greater",
                            self.cluster_name, self.node_id,
                        );
                    }
                };

                match connection {
                    ConnOrConn::Connection(connection) => {
                        match open_channels(
                            connection,
                            last_connection_id,
                            &self.cluster_name,
                            &self.node_id,
                        )
                        .await
                        {
                            Ok(own_connection) => insert_connection(own_connection),
                            Err(err) => {
                                eprintln!(
                                    "ERROR cluster {}: trying to initialize connection to node {}: {err}",
                                    self.cluster_name, self.node_id,
                                );
                                if self.connection.is_none() {
                                    self.ask_connection();
                                }
                            }
                        }
                    }
                    ConnOrConn::OwnConnection(own_connection) => insert_connection(own_connection),
                }
            }
        }
    }

    async fn handle_remote_message(&mut self, message: MaybeRemoteLine) {
        match message {
            Err(err) => {
                eprintln!(
                    "WARN cluster {}: node {}: connection {}: error receiving remote message: {err}",
                    self.cluster_name, self.node_id, self.last_connection_id
                );
                self.close_connection(CLOSE_RECV).await;
            }
            Ok(None) => {
                eprintln!(
                    "WARN cluster {}: node {} closed its stream",
                    self.cluster_name, self.node_id,
                );
                self.close_connection(CLOSE_CLOSED).await;
            }
            Ok(Some(line)) => {
                if let Err(err) = self
                    .own_cluster_tx
                    .send((line.0.clone(), line.1.into()))
                    .await
                {
                    eprintln!(
                        "ERROR cluster {}: could not send message to reaction stream: {err}",
                        self.cluster_name
                    );
                    eprintln!(
                        "INFO cluster {}: line that can't be sent: {}",
                        self.cluster_name, line.0
                    );
                    self.quit();
                } else {
                    eprintln!(
                        "DEBUG cluster {}: node {}: sent a remote message to local stream: {}",
                        self.cluster_name, self.node_id, line.0
                    );
                }
            }
        }
    }

    async fn handle_local_message(&mut self, message: Option<UtcLine>) {
        eprintln!(
            "DEBUG cluster {}: node {}: received a local message",
            self.cluster_name, self.node_id,
        );
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
                    if let Err(err) = connection.send_line(&message.0, &message.1).await {
                        eprintln!(
                            "INFO cluster {}: connection with node {} failed: {err}",
                            self.cluster_name, self.node_id,
                        );
                        self.message_queue.insert(message.1, message.0).await;
                        self.close_connection(CLOSE_SEND).await;
                    } else {
                        eprintln!(
                            "DEBUG cluster {}: node {}: sent a local message to remote: {}",
                            self.cluster_name, self.node_id, message.0
                        );
                    }
                }
                None => {
                    eprintln!(
                        "DEBUG cluster {}: node {}: no connection, saving local message to send later: {}",
                        self.cluster_name, self.node_id, message.0
                    );
                    self.message_queue.insert(message.1, message.0).await;
                }
            },
        }
    }

    async fn close_connection(&mut self, code: (u32, &[u8])) {
        if let Some(connection) = self.connection.take() {
            connection
                .connection
                .close(VarInt::from_u32(code.0), code.1);
        }
        self.ask_connection();
    }

    fn ask_connection(&mut self) {
        // if self.node_id.starts_with('H') {
        let (tx, rx) = mpsc::channel(1);
        self.cancel_ask_connection = Some(tx);
        try_connect(
            self.cluster_name.clone(),
            self.remote.clone(),
            self.endpoint.clone(),
            self.last_connection_id,
            self.connection_tx.clone(),
            rx,
        );
    }

    fn quit(&mut self) {
        self.shutdown.ask_shutdown();
    }
}

/// Open accept one stream and create one stream.
/// This way, there is no need to know if we created or accepted the connection.
async fn open_channels(
    connection: Connection,
    last_connexion_id: u64,
    cluster_name: &str,
    node_id: &str,
) -> Result<OwnConnection, IoError> {
    eprintln!(
        "DEBUG cluster {}: node {}: opening uni channel",
        cluster_name, node_id
    );
    let mut output = BufWriter::new(connection.open_uni().await?);

    let our_id = random_range(last_connexion_id + 1..last_connexion_id + 1_000_000);

    eprintln!(
        "DEBUG cluster {}: node {}: sending handshake in uni channel",
        cluster_name, node_id
    );
    output.write_u32(PROTOCOL_VERSION).await?;
    output.write_u64(our_id).await?;
    output.flush().await?;

    eprintln!(
        "DEBUG cluster {}: node {}: accepting uni channel",
        cluster_name, node_id
    );
    let mut input = BufReader::new(connection.accept_uni().await?);

    eprintln!(
        "DEBUG cluster {}: node {}: reading handshake from uni channel",
        cluster_name, node_id
    );
    let their_version = input.read_u32().await?;

    if their_version != PROTOCOL_VERSION {
        return Err(IoError::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "incompatible version: {their_version}. We use {PROTOCOL_VERSION}. Consider upgrading the node with the older version."
            ),
        ));
    }

    let their_id = input.read_u64().await?;
    // FIXME Do we need to test this? If so, this function should return their_id even when error in order to retry better next time
    // if their_id < last_connexion_id
    // ERROR
    // else
    let chosen_id = max(our_id, their_id);
    eprintln!(
        "DEBUG cluster {}: node {}: version handshake complete: last id: {last_connexion_id}, our id: {our_id}, their id: {their_id}: chosen id: {chosen_id}",
        cluster_name, node_id
    );
    Ok(OwnConnection::new(connection, chosen_id, output, input))
}

async fn false_recv() -> MaybeRemoteLine {
    Ok(None)
}

const START_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_TIMEOUT: Duration = Duration::from_hours(1);
const TIMEOUT_FACTOR: f64 = 1.5;

fn with_random(d: Duration) -> Duration {
    let max_delta = d.as_micros() as f32 * 0.2;
    d + Duration::from_micros(rand::random_range(0.0..max_delta) as u64)
}

// Compute the next wait Duration.
// We're multiplying the Duration by [`TIMEOUT_FACTOR`] and cap it to [`MAX_TIMEOUT`].
fn next_delta(delta: Option<Duration>) -> Duration {
    with_random(match delta {
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
    })
}

#[cfg(test)]
#[test]
fn test_with_random() {
    for d in [
        123, 1234, 12345, 123456, 1234567, 12345678, 123456789, 1234567890,
    ] {
        let rd = with_random(Duration::from_micros(d)).as_micros();
        assert!(rd as f32 >= d as f32, "{rd} < {d}");
        assert!(rd as f32 <= (d + 1) as f32 * 1.2, "{rd} > {d} * 1.2");
    }
}

fn try_connect(
    cluster_name: String,
    remote: EndpointAddr,
    endpoint: Arc<Endpoint>,
    last_connection_id: u64,
    connection_tx: mpsc::Sender<ConnOrConn>,
    mut order_stop: mpsc::Receiver<()>,
) {
    tokio::spawn(async move {
        let node_id = remote.id.show();
        // Until we have a connection or we're requested to stop
        let mut keep_trying = true;
        let mut delta = None;

        while keep_trying {
            delta = Some(next_delta(delta));

            keep_trying = tokio::select! {
                _ = sleep(delta.unwrap_or_default()) => true,
                _ = order_stop.recv() => false,
            };
            if keep_trying {
                eprintln!("DEBUG cluster {cluster_name}: node {node_id}: trying to connect...");
                let connect = tokio::select! {
                    conn = endpoint.connect(remote.clone(), ALPN[0]) => Some(conn),
                    _ = order_stop.recv() => None,
                };
                if let Some(connect) = connect {
                    let res = match connect {
                        Ok(connection) => {
                            eprintln!(
                                "DEBUG cluster {cluster_name}: node {node_id}: created connection"
                            );
                            match open_channels(
                                connection,
                                last_connection_id,
                                &cluster_name,
                                &node_id,
                            )
                            .await
                            {
                                Ok(own_connection) => {
                                    if let Err(err) = connection_tx
                                        .send(ConnOrConn::OwnConnection(own_connection))
                                        .await
                                    {
                                        eprintln!(
                                            "DEBUG cluster {cluster_name}: node {node_id}: quitting because ConnectionManager has quit: {err}"
                                        );
                                    }
                                    // successfully opened connection
                                    keep_trying = false;
                                    Ok(())
                                }
                                Err(err) => Err(err.to_string()),
                            }
                        }
                        Err(err) => Err(err.to_string()),
                    };
                    if let Err(err) = res {
                        eprintln!(
                            "WARN cluster {cluster_name}: node {node_id}: while trying to connect: {err}"
                        );
                    }
                } else {
                    // received stop order
                    eprintln!(
                        "DEBUG cluster {cluster_name}: node {node_id}: stop to try connecting to node be cause we received a connection from it"
                    );
                    keep_trying = false;
                }
            }
        }
    });
}
