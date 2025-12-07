use std::{
    cmp::max,
    collections::VecDeque,
    io::Error as IoError,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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
};

use crate::{
    cluster::{ALPN, UtcLine, connect_config},
    key::Show,
};

// const START_TIMEOUT: Duration = Duration::from_secs(5);
// const MAX_TIMEOUT: Duration = Duration::from_secs(60 * 60); // 1 hour
// const TIMEOUT_FACTOR: f64 = 1.5;

const PROTOCOL_VERSION: u32 = 1;

type RemoteLine = (String, Duration);
type MaybeRemoteLine = Result<Option<RemoteLine>, IoError>;

enum Event {
    LocalMessageReceived(Option<UtcLine>),
    RemoteMessageReceived(MaybeRemoteLine),
    ConnectionReceived(Option<Connection>),
}

enum Insert {
    Front,
    Back,
}

struct OwnConnection {
    connection: Connection,

    line_tx: BufWriter<SendStream>,
    line_rx: BufReader<RecvStream>,

    next_time: Option<Duration>,
    next_len: Option<usize>,
    next_line: Option<Vec<u8>>,
}

impl OwnConnection {
    fn new(
        connection: Connection,
        line_tx: BufWriter<SendStream>,
        line_rx: BufReader<RecvStream>,
    ) -> Self {
        Self {
            connection,
            line_tx,
            line_rx,
            next_time: None,
            next_len: None,
            next_line: None,
        }
    }

    /// Send a line to peer
    async fn send_line(&mut self, line: RemoteLine) -> Result<(), std::io::Error> {
        self.line_tx.write_u64(line.1.as_micros() as u64).await?;
        self.line_tx.write_u32(line.0.len() as u32).await?;
        self.line_tx.write_all(line.0.as_bytes()).await?;
        self.line_tx.flush().await?;
        Ok(())
    }

    /// Cancel-safe function that returns next line from peer
    /// Returns None if we don't have all data yet.
    async fn recv_line(&mut self) -> Result<Option<RemoteLine>, std::io::Error> {
        if self.next_time.is_none() {
            self.next_time = Some(Duration::from_micros(self.line_rx.read_u64().await?));
        }
        // Ok we have next_time.is_some()
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
            Ok(Some((line, self.next_time.take().unwrap())))
        } else {
            // Ok we don't have a full line, will be next time!
            Ok(None)
        }
    }
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
    message_timeout: Duration,
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
        message_timeout: Duration,
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
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
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
                match open_channels(connection, self.last_connexion_id).await {
                    Ok((own_connection, new_id)) => {
                        if self.connection.is_none() || self.last_connexion_id < new_id {
                            self.connection = Some(own_connection);
                            self.last_connexion_id = new_id;
                        } else {
                            eprintln!(
                                "WARN cluster {}: node {}: ignoring incoming connection, as we already have a valid connection with it and our connection id is greater",
                                self.cluster_name, self.node_id,
                            );
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

    async fn handle_remote_message(&mut self, message: MaybeRemoteLine) {
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
            Ok(Some(line)) => {
                // TODO from_utc
                let local_time = line.1;
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
                    // TODO to_utc
                    if let Err(err) = connection.send_line((message.0.clone(), message.1)).await {
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
        if let Some(connection) = self.connection.take() {
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

/// Open accept one stream and create one stream.
/// This way, there is no need to know if we created or accepted the connection.
async fn open_channels(
    connection: Connection,
    last_connexion_id: u64,
) -> Result<(OwnConnection, u64), IoError> {
    eprintln!("DEBUG opening uni channel");
    let mut output = BufWriter::new(connection.open_uni().await?);

    let our_id = random_range(last_connexion_id + 1..last_connexion_id + 1_000_000);

    eprintln!("DEBUG sending handshake in uni channel");
    output.write_u32(PROTOCOL_VERSION).await?;
    output.write_u64(our_id).await?;
    output.flush().await?;

    eprintln!("DEBUG accepting uni channel");
    let mut input = BufReader::new(connection.accept_uni().await?);

    eprintln!("DEBUG reading handshake from uni channel");
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
        "DEBUG version handshake complete: last id: {last_connexion_id}, our id: {our_id}, their id: {their_id}: chosen id: {chosen_id}"
    );
    Ok((OwnConnection::new(connection, output, input), chosen_id))
}

async fn false_recv() -> MaybeRemoteLine {
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
                    // conn = endpoint.connect(remote.clone(), ALPN[0]) => Some(conn),
                    conn = endpoint.connect_with_opts(remote.clone(), ALPN[0], connect_config()) => Some(conn),
                    _ = order_stop.recv() => None,
                };
                if let Some(connect) = connect {
                    match connect {
                        Ok(connecting) => match connecting.await {
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
                        },
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
