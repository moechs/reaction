use std::{collections::BTreeMap, fs, io, path::PathBuf, process::exit, sync::Arc};

use bincode::Options;
use futures::{SinkExt, StreamExt};
use regex::Regex;
use tokio::{
    join,
    net::UnixListener,
    sync::{mpsc, oneshot, watch},
};
use tokio_util::{
    bytes::Bytes,
    codec::{Framed, LengthDelimitedCodec},
};
use tracing::{error, warn};

use crate::{
    concepts::{
        ActionFilter, ClientRequest, ClientStatus, Config, DaemonResponse, Order, Pattern,
        PatternStatus,
    },
    utils::bincode_options,
};

use super::{execs::ExecsMap, matches::MatchesMap, statemap::FilterOptions};

macro_rules! err_str {
    ($expression:expr) => {
        $expression.map_err(|err| err.to_string())
    };
}

fn open_socket(path: PathBuf) -> Result<UnixListener, String> {
    // First create all directories to the file
    let dir = path
        .parent()
        .ok_or(format!("socket {path:?} has no parent directory"))?;
    err_str!(fs::create_dir_all(dir))?;
    // Test if file exists
    match fs::metadata(&path) {
        Ok(meta) => {
            if meta.file_type().is_dir() {
                Err(format!("socket {path:?} is already a directory"))
            } else {
                warn!("socket {path:?} already exists: is the daemon already running? deleting.");
                err_str!(fs::remove_file(&path))
            }
        }
        Err(err) => err_str!(match err.kind() {
            io::ErrorKind::NotFound => Ok(()),
            _ => Err(err),
        }),
    }?;
    // Open socket
    err_str!(UnixListener::bind(path))
}

async fn answer_order(
    config: &'static Config,
    match_tx: &mpsc::Sender<SocketOrder<MatchesMap>>,
    exec_tx: &mpsc::Sender<SocketOrder<ExecsMap>>,
    options: ClientRequest,
) -> Result<ClientStatus, String> {
    // Compute options
    let filtering_options = {
        let (stream_name, filter_name) = match options.stream_filter {
            Some(sf) => match sf.split_once(".") {
                Some((s, f)) => (Some(s.to_string()), Some(f.to_string())),
                None => (Some(sf), None),
            },
            None => (None, None),
        };

        // Compute the Vec<(pattern_name, String)> into a BTreeMap<Arc<Pattern>, Regex>
        let patterns = options
            .patterns
            .iter()
            .map(|(name, reg)| {
                // lookup pattern in config.patterns
                config
                    .patterns()
                    .iter()
                    // retrieve or Err
                    .find(|(pattern_name, _)| name == *pattern_name)
                    .ok_or_else(|| format!("pattern '{name}' doesn't exist"))
                    // compile Regex or Err
                    .and_then(|(_, pattern)| match Regex::new(reg) {
                        Ok(reg) => Ok((pattern.clone(), reg)),
                        Err(err) => Err(format!("pattern '{name}' regex doesn't compile: {err}")),
                    })
            })
            .collect::<Result<BTreeMap<Arc<Pattern>, Regex>, String>>()?;

        FilterOptions {
            stream_name,
            filter_name,
            patterns,
        }
    };

    // ask for matches clone
    let filtering_options2 = filtering_options.clone();
    let matches = async move {
        let (m_tx, m_rx) = oneshot::channel();

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        match_tx
            .send((options.order, filtering_options2, m_tx))
            .await
            .unwrap();

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        m_rx.await.unwrap()
    };

    // ask for execs clone
    let execs = async move {
        let (e_tx, e_rx) = oneshot::channel();

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        exec_tx
            .send((options.order, filtering_options, e_tx))
            .await
            .unwrap();

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        e_rx.await.unwrap()
    };

    let (matches, execs) = join!(matches, execs);

    // Transform matches and execs into a ClientStatus
    let cs: ClientStatus = matches
        .into_iter()
        .fold(BTreeMap::new(), |mut acc, (object, map)| {
            let (stream, filter, _) = object.full_name();
            acc.entry(stream.into())
                .or_default()
                .entry(filter.into())
                .or_default()
                .extend(map.into_iter().map(|(match_, times)| {
                    (
                        match_.join(" "),
                        PatternStatus {
                            matches: times.len(),
                            ..Default::default()
                        },
                    )
                }));
            acc
        });

    let cs = execs.into_iter().fold(cs, |mut acc, (object, map)| {
        let (stream, filter, action) = object.full_name();
        let inner_map = acc
            .entry(stream.into())
            .or_default()
            .entry(filter.into())
            .or_default();

        map.into_iter().for_each(|(match_, times)| {
            inner_map
                .entry(match_.join(" "))
                .or_default()
                .actions
                .insert(
                    action.to_string(),
                    times
                        .into_iter()
                        .map(|time| time.to_rfc3339().chars().take(19).collect())
                        .collect(),
                );
        });
        acc
    });

    Ok(cs)
}

macro_rules! or_next {
    ($msg:expr, $expression:expr) => {
        match $expression {
            Ok(x) => x,
            Err(err) => {
                error!("failed to answer client: {}, {}", $msg, err);
                continue;
            }
        }
    };
}

pub type SocketOrder<T> = (Order, FilterOptions, oneshot::Sender<T>);

pub async fn socket_manager(
    config: &'static Config,
    socket: PathBuf,
    match_tx: mpsc::Sender<SocketOrder<MatchesMap>>,
    exec_tx: mpsc::Sender<SocketOrder<ExecsMap>>,
    mut stop: watch::Receiver<bool>,
) {
    let listener = match open_socket(socket) {
        Ok(l) => l,
        Err(err) => {
            error!("while creating communication socket: {err}");
            exit(1);
        }
    };

    let bin = bincode_options();
    loop {
        tokio::select! {
            _ = stop.changed() => {
                break
            }
            try_conn = listener.accept() => {
                match try_conn {
                    Ok((conn, _)) => {
                        let mut transport = Framed::new(conn, LengthDelimitedCodec::new());
                        // Decode
                        let received = transport.next().await;
                        let encoded_request = match received {
                            Some(r) => or_next!("while reading request", r),
                            None => {
                                error!("failed to answer client: client sent no request");
                                continue;
                            }
                        };
                        let request = or_next!(
                            "failed to decode request",
                            bin.deserialize(&encoded_request)
                            );
                        // Process
                        let response = match answer_order(config, &match_tx, &exec_tx, request).await {
                            Ok(res) => DaemonResponse::Order(res),
                            Err(err) => DaemonResponse::Err(err),
                        };
                        // Encode
                        let encoded_response =
                            or_next!("failed to serialize response", bin.serialize(&response));
                        or_next!(
                            "failed to send response:",
                            transport.send(Bytes::from(encoded_response)).await
                            );
                    }
                    Err(err) => error!("failed to open connection from cli: {err}"),
                }
            }
        }
    }
}
