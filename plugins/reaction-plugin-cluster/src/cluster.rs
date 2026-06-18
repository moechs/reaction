use std::{
    collections::BTreeMap,
    net::{SocketAddrV4, SocketAddrV6},
    sync::Arc,
};

use futures::future::join_all;
use iroh::{Endpoint, endpoint::presets};
use reaction_plugin::{Line, shutdown::ShutdownController};
use remoc::rch::mpsc as remocMpsc;
use tokio::sync::mpsc as tokioMpsc;
use treedb::{Database, time::Time};

use crate::{ActionInit, StreamInit, connection::ConnectionManager, endpoint::EndpointManager};

pub const ALPN: [&[u8]; 1] = ["reaction_cluster_1".as_bytes()];

pub type UtcLine = (Arc<String>, Time);

// pub fn transport_config() -> TransportConfig {
//     // FIXME higher timeouts and keep alive
//     let mut transport = TransportConfig::default();
//     transport
//         .max_idle_timeout(Some(Duration::from_millis(2000).try_into().unwrap()))
//         .keep_alive_interval(Some(Duration::from_millis(200)));
//     transport
// }

// pub fn connect_config() -> ConnectOptions {
//     ConnectOptions::new().with_transport_config(transport_config().into())
// }

pub async fn bind(stream: &StreamInit) -> Result<Endpoint, String> {
    let mut builder = Endpoint::builder(presets::Empty)
        .secret_key(stream.secret_key.clone())
        .alpns(ALPN.iter().map(|slice| slice.to_vec()).collect())
        .relay_mode(iroh::RelayMode::Disabled);

    if let Some(ip) = stream.bind_ipv4 {
        builder = builder
            .bind_addr(SocketAddrV4::new(ip, stream.listen_port))
            .map_err(|err| err.to_string())?;
    }
    if let Some(ip) = stream.bind_ipv6 {
        builder = builder
            .bind_addr(SocketAddrV6::new(ip, stream.listen_port, 0, 0))
            .map_err(|err| err.to_string())?;
    }

    builder.bind().await.map_err(|err| {
        format!(
            "Could not create socket address for cluster {}: {err}",
            stream.name
        )
    })
}

pub async fn cluster_tasks(
    endpoint: Endpoint,
    mut stream: StreamInit,
    mut actions: Vec<ActionInit>,
    db: &mut Database,
    shutdown: ShutdownController,
) -> Result<(), String> {
    eprintln!("DEBUG cluster tasks starts running");

    let (message_action2connection_txs, mut message_action2connection_rxs): (
        Vec<tokioMpsc::Sender<UtcLine>>,
        Vec<tokioMpsc::Receiver<UtcLine>>,
    ) = (0..stream.nodes.len())
        .map(|_| tokioMpsc::channel(1))
        .unzip();

    // Spawn action tasks
    while let Some(mut action) = actions.pop() {
        let message_action2connection_txs = message_action2connection_txs.clone();
        let own_cluster_tx = stream.tx.clone();
        tokio::spawn(async move {
            action
                .serve(message_action2connection_txs, own_cluster_tx)
                .await
        });
    }

    let endpoint = Arc::new(endpoint);

    let mut connection_endpoint2connection_txs = BTreeMap::new();

    // Spawn connection managers
    while let Some((pk, endpoint_addr)) = stream.nodes.pop_first() {
        let cluster_name = stream.name.clone();
        let endpoint = endpoint.clone();
        let message_action2connection_rx = message_action2connection_rxs.pop().unwrap();
        let stream_tx = stream.tx.clone();
        let shutdown = shutdown.clone();
        let (connection_manager, connection_endpoint2connection_tx) = ConnectionManager::new(
            cluster_name,
            endpoint_addr,
            endpoint,
            stream.message_timeout,
            message_action2connection_rx,
            stream_tx,
            db,
            shutdown,
        )
        .await?;
        tokio::spawn(async move { connection_manager.task().await });
        connection_endpoint2connection_txs.insert(pk, connection_endpoint2connection_tx);
    }

    // Spawn connection accepter
    EndpointManager::new(
        endpoint.clone(),
        stream.name.clone(),
        connection_endpoint2connection_txs,
        shutdown.clone(),
    );

    eprintln!("DEBUG cluster tasks finished running");
    Ok(())
}

impl ActionInit {
    // Receive messages from its reaction action and dispatch them to all connections and to the reaction stream
    async fn serve(
        &mut self,
        nodes_tx: Vec<tokioMpsc::Sender<UtcLine>>,
        own_stream_tx: remocMpsc::Sender<Line>,
    ) {
        while let Ok(Some(m)) = self.rx.recv().await {
            eprintln!("DEBUG action: received a message to send to connections");
            let line = self.send.line(m.match_);
            if self.self_
                && let Err(err) = own_stream_tx.send((line.clone(), m.time)).await
            {
                eprintln!("ERROR while queueing message to be sent to own cluster stream: {err}");
            }

            let line = (Arc::new(line), m.time.into());
            for result in join_all(nodes_tx.iter().map(|tx| tx.send(line.clone()))).await {
                if let Err(err) = result {
                    eprintln!("ERROR while queueing message to be sent to cluster nodes: {err}");
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Local};

    // As long as nodes communicate with UTC datetimes, them having different local timezones is not an issue!
    #[test]
    fn different_local_tz_is_ok() {
        let dates: Vec<DateTime<Local>> = serde_json::from_str(
            "[\"2025-11-02T17:47:21.716229569+01:00\",\"2025-11-02T18:47:21.716229569+02:00\"]",
        )
        .unwrap();

        assert_eq!(dates[0].to_utc(), dates[1].to_utc());
    }
}
