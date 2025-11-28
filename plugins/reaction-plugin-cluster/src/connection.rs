use std::{cmp::max, collections::VecDeque, sync::Arc};

use chrono::{DateTime, Local, TimeDelta, Utc};
use futures::FutureExt;
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::{Connection, VarInt},
};
use rand::random_range;
use reaction_plugin::{Line, shutdown::ShutdownController};
use remoc::{Connect, rch::base};
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, sync::mpsc};

use crate::{
    cluster::{ALPN, UtcLine},
    key::Show,
};

// const START_TIMEOUT: Duration = Duration::from_secs(5);
// const MAX_TIMEOUT: Duration = Duration::from_secs(60 * 60); // 1 hour
// const TIMEOUT_FACTOR: f64 = 1.5;

const PROTOCOL_VERSION: u32 = 1;

enum Event {
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
    /// The remote node we're communicating with (for logging)
    node_id: String,

    /// Ask for a connection
    ask_connection: mpsc::Sender<mpsc::Receiver<()>>,
    cancel_ask_connection: Option<mpsc::Sender<()>>,
    /// The EndpointManager sending us a connection (whether we asked for it or not)
    connection_rx: mpsc::Receiver<Connection>,
    /// Our own connection (when we have one)
    connection: Option<OwnConnection>,
    /// Last connexion ID, used to have a determinist way to choose between conflicting connections
    last_connexion_id: u64,

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
        connection_tx: mpsc::Sender<Connection>,
        connection_rx: mpsc::Receiver<Connection>,
        message_timeout: TimeDelta,
        message_rx: mpsc::Receiver<UtcLine>,
        own_cluster_tx: remoc::rch::mpsc::Sender<Line>,
        shutdown: ShutdownController,
    ) -> Self {
        let node_id = remote.id.show();
        let (ask_connection, order_start) = mpsc::channel(1);
        try_connect(
            cluster_name.clone(),
            remote.clone(),
            endpoint.clone(),
            connection_tx,
            order_start,
        );
        Self {
            cluster_name,
            node_id,
            connection: None,
            ask_connection,
            cancel_ask_connection: None,
            connection_rx,
            last_connexion_id: 0,
            message_timeout,
            message_rx,
            message_queue: VecDeque::default(),
            own_cluster_tx,
            shutdown,
        }
    }

    /// Main loop
    pub async fn task(mut self) {
        self.ask_connection().await;
        loop {
            eprintln!("DEBUG connection: NEW LOOP!");

            let have_connection = self.connection.is_some();
            let maybe_conn_rx = self
                .connection
                .as_mut()
                .map(|conn| conn.rx.recv().boxed())
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
                self.handle_local_message(utc_line, Insert::Back).await;
            }
            Event::RemoteMessageReceived(message) => {
                self.handle_remote_message(message).await;
            }
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
                self.cluster_name, self.node_id,
            )
        }
    }

    /// Bootstrap a new Connection
    /// Returns true if we have a valid connection now
    async fn handle_connection(&mut self, connection: Option<Connection>) {
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
                match open_channels(&connection, self.last_connexion_id).await {
                    Ok((mut tx, rx, new_id)) => {
                        if self.connection.is_none() || self.last_connexion_id < new_id {
                            if let Some(mut conn) = self.connection.take() {
                                // FIXME should we do this in a separate task?
                                let _ = conn.tx.send(RemoteMessage::Quitting).await;
                            }
                            self.connection = Some(OwnConnection { connection, tx, rx });
                            self.last_connexion_id = new_id;
                        } else {
                            eprintln!(
                                "WARN cluster {}: node {}: ignoring incoming connection, as we already have a valid connection with it and our connection id is greater",
                                self.cluster_name, self.node_id,
                            );
                            // FIXME should we do this in a separate task?
                            let _ = tx.send(RemoteMessage::Quitting).await;
                        }
                    }
                    Err(err) => {
                        eprintln!(
                            "ERROR cluster {}: trying to initialize connection to node {}: {err}",
                            self.cluster_name, self.node_id,
                        );
                        if self.connection.is_none() {
                            self.ask_connection().await;
                        }
                    }
                }
            }
        }
    }

    async fn handle_remote_message(
        &mut self,
        message: Result<Option<RemoteMessage>, remoc::rch::base::RecvError>,
    ) {
        match message {
            Err(err) => {
                eprintln!(
                    "WARN cluster {}: node {}: connection {}: error receiving remote message: {err}",
                    self.cluster_name, self.node_id, self.last_connexion_id
                );
                self.close_connection(1, b"error receiving from your stream")
                    .await;
            }
            Ok(None) => {
                eprintln!(
                    "WARN cluster {}: node {} closed its stream",
                    self.cluster_name, self.node_id,
                );
                self.close_connection(1, b"you closed your stream").await;
            }
            Ok(Some(RemoteMessage::Handshake(_, _))) => {
                eprintln!(
                    "WARN cluster {}: node {} sent invalid message, ignoring",
                    self.cluster_name, self.node_id,
                );
            }
            Ok(Some(RemoteMessage::Quitting)) => {
                eprintln!(
                    "INFO cluster {}: node {} is quitting, bye bye",
                    self.cluster_name, self.node_id,
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
                } else {
                    eprintln!(
                        "DEBUG cluster {}: node {}: sent a remote message to local stream: {}",
                        self.cluster_name, self.node_id, line.0
                    );
                }
            }
        }
    }

    async fn handle_local_message(&mut self, message: Option<UtcLine>, insert: Insert) {
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
                            self.cluster_name, self.node_id,
                        );
                        self.message_queue.push_back(message);
                        self.close_connection(
                            0,
                            b"could not send a message to your channel so I quit",
                        )
                        .await;
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
        self.ask_connection().await;
    }

    async fn ask_connection(&mut self) {
        // if self.node_id.starts_with('H') {
        let (tx, rx) = mpsc::channel(1);
        self.cancel_ask_connection = Some(tx);
        if let Err(err) = self.ask_connection.send(rx).await {
            eprintln!(
                "ERROR cluster {}: node {}: quitting because our connection initiater quitted: {}",
                self.cluster_name, self.node_id, err
            );
            self.quit();
        }
        // }
    }

    fn quit(&mut self) {
        self.shutdown.ask_shutdown();
    }
}

/// Compute the next wait Duration.
/// We're multiplying the Duration by [`TIMEOUT_FACTOR`] and cap it to [`MAX_TIMEOUT`].
// fn next_delta(delta: Option<Duration>) -> Duration {
//     match delta {
//         None => START_TIMEOUT,
//         Some(delta) => {
//             // Multiply timeout by TIMEOUT_FACTOR
//             let delta = Duration::from_millis(((delta.as_millis() as f64) * TIMEOUT_FACTOR) as u64);
//             // Cap to MAX_TIMEOUT
//             if delta > MAX_TIMEOUT {
//                 MAX_TIMEOUT
//             } else {
//                 delta
//             }
//         }
//     }
// }

/// All possible communication messages
/// Set as an enum for forward compatibility
#[derive(Serialize, Deserialize)]
pub enum RemoteMessage {
    /// Must be the first message sent over, then should not be sent again
    ///
    /// u32 is a version number.
    /// The version must be the same, or compatibility is broken.
    ///
    /// u64 is a connection id.
    /// Each node sends a number between n and n + 1_000_000.
    /// Both nodes keep the highest of both numbers.
    /// n starts at 0.
    /// When recreating a new connection, n starts at _last connection number_ + 1.
    /// This leaves room for a minimum of 18_446_744_073_709 connections between two nodes
    /// For each "shared uptime", which I hope is sufficient :p
    /// Will be reset as soon as there is an instant where both nodes are down
    ///
    /// This protocols permits to determine on both nodes which connection to drop
    /// when we have one connection and receive another.
    Handshake(u32, u64),
    /// A line to transmit to your stream
    Line((String, DateTime<Utc>)),
    /// Announce the node is closing
    Quitting,
}

/// Open accept one stream and create one stream.
/// This way, there is no need to know if we created or accepted the connection.
async fn open_channels(
    connection: &Connection,
    last_connexion_id: u64,
) -> Result<
    (
        base::Sender<RemoteMessage>,
        base::Receiver<RemoteMessage>,
        u64,
    ),
    String,
> {
    eprintln!("DEBUG opening uni channel");
    let mut output = connection.open_uni().await.map_err(|err| err.to_string())?;

    eprintln!("DEBUG sending 1 byte in uni channel");
    let res = output.write(&[0; 1]).await.map_err(|err| err.to_string())?;
    eprintln!("DEBUG sent {res} byte in uni channel");
    output.flush().await.map_err(|err| err.to_string())?;

    eprintln!("DEBUG accepting uni channel");
    let mut input = connection
        .accept_uni()
        .await
        .map_err(|err| err.to_string())?;

    eprintln!("DEBUG reading 1 byte from uni channel");
    input
        .read(&mut [0; 1])
        .await
        .map_err(|err| err.to_string())?;

    eprintln!("DEBUG creating remoc channels");
    let (conn, mut tx, mut rx) = Connect::io_buffered(remoc::Cfg::default(), input, output, 1024)
        .await
        .map_err(|err| err.to_string())?;

    tokio::spawn(conn);

    let our_id = random_range(last_connexion_id + 1..last_connexion_id + 1_000_000);

    eprintln!("DEBUG sending version");
    tx.send(RemoteMessage::Handshake(PROTOCOL_VERSION, our_id))
        .await
        .map_err(|err| err.to_string())?;

    eprintln!("DEBUG receiving version");
    match rx.recv().await {
        // Good protocol version!
        Ok(Some(RemoteMessage::Handshake(PROTOCOL_VERSION, their_id))) => {
            // FIXME Do we need to test this? If so, this function should return their_id even when error in order to retry better next time
            // if their_id < last_connexion_id
            // ERROR
            // else
            let chosen_id = max(our_id, their_id);
            eprintln!(
                "DEBUG version handshake complete: last id: {last_connexion_id}, our id: {our_id}, their id: {their_id}: chosen id: {chosen_id}"
            );
            Ok((tx, rx, chosen_id))
        }
        // Errors
        Ok(Some(RemoteMessage::Handshake(other, _))) => Err(format!(
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

async fn false_recv() -> Result<Option<RemoteMessage>, remoc::rch::base::RecvError> {
    Ok(None)
}

fn try_connect(
    cluster_name: String,
    remote: EndpointAddr,
    endpoint: Arc<Endpoint>,
    connection_tx: mpsc::Sender<Connection>,
    mut order_start: mpsc::Receiver<mpsc::Receiver<()>>,
) {
    tokio::spawn(async move {
        let node_id = remote.id.show();
        // Each time we receive and order
        while let Some(mut order_stop) = order_start.recv().await {
            // Until we have a connection or we're requested to stop
            let mut keep_trying = true;
            while keep_trying {
                eprintln!("DEBUG cluster {cluster_name}: node {node_id}: trying to connect...");
                let connect = tokio::select! {
                    conn = endpoint.connect(remote.clone(), ALPN[0]) => Some(conn),
                    _ = order_stop.recv() => None,
                };
                if let Some(connect) = connect {
                    match connect {
                        Ok(connection) => {
                            eprintln!(
                                "DEBUG cluster {cluster_name}: node {node_id}: created connection"
                            );
                            if let Err(err) = connection_tx.send(connection).await {
                                eprintln!(
                                    "DEBUG cluster {cluster_name}: node {node_id}: quitting because ConnectionManager has quit: {err}"
                                );
                                order_start.close();
                            }
                            keep_trying = false;
                        }
                        Err(err) => {
                            eprintln!(
                                "WARN cluster {cluster_name}: node {node_id}: while trying to connect: {err}"
                            );
                        }
                    }
                } else {
                    // received stop order
                    keep_trying = false;
                }
            }
        }
    });
}
