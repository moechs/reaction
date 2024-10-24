use std::{collections::BTreeMap, fs, io, path::PathBuf, process::exit, sync::Arc};

use bincode::Options;
use futures::{SinkExt, StreamExt};
use regex::Regex;
use tokio::{net::UnixListener, sync::broadcast};
use tokio_util::{
    bytes::Bytes,
    codec::{Framed, LengthDelimitedCodec},
};
use tracing::{error, warn};

use crate::{
    concepts::{ClientRequest, ClientStatus, Config, DaemonResponse, Pattern},
    utils::bincode_options,
};

use super::SharedState;

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
    shared_state: &Arc<SharedState>,
    options: ClientRequest,
) -> Result<ClientStatus, String> {
    // Compute options
    let (stream_name, filter_name) = match options.stream_filter {
        Some(sf) => match sf.split_once(".") {
            Some((s, f)) => (Some(s.to_string()), Some(f.to_string())),
            None => (Some(sf), None),
        },
        None => (None, None),
    };

    // Compute the Vec<(pattern_name: String, regex: String)> into a BTreeMap<Arc<Pattern>, Regex>
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

    let cs: ClientStatus = futures::stream::iter(
        shared_state
            .s
            .iter()
            // stream filtering
            .filter(|(stream, _)| {
                stream_name.is_none()
                    || stream_name
                        .clone()
                        .is_some_and(|name| name == stream.name())
            }),
    )
    .fold(BTreeMap::new(), |mut acc, (stream, filter_manager)| async {
        let mut filter_manager = filter_manager.lock().await;
        let inner_map = futures::stream::iter(
            filter_manager
                .iter_mut()
                // filter filtering
                .filter(|(filter, _)| {
                    filter_name.is_none()
                        || filter_name
                            .clone()
                            .is_some_and(|name| name == filter.name())
                })
                // pattern filtering
                .filter(|(filter, _)| {
                    patterns
                        .iter()
                        .all(|(pattern, _)| filter.patterns().get(pattern).is_some())
                }),
        )
        .then(|(filter, manager)| async {
            (
                filter.name().to_owned(),
                manager.handle_order(&patterns, options.order).await,
            )
        })
        .collect()
        .await;
        acc.insert(stream.name().to_owned(), inner_map);
        acc
    })
    .await;

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

pub async fn socket_manager(
    config: &'static Config,
    socket: PathBuf,
    shared_state: Arc<SharedState>,
    mut stop: broadcast::Receiver<()>,
) {
    let listener = match open_socket(socket.clone()) {
        Ok(l) => l,
        Err(err) => {
            error!("while creating communication socket: {err}");
            exit(1);
        }
    };

    let bin = bincode_options();
    loop {
        tokio::select! {
            _ = stop.recv() => break,
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
                        let response = match answer_order(config, &shared_state, request).await {
                            Ok(res) => DaemonResponse::Order(res),
                            Err(err) => DaemonResponse::Err(err),
                        };
                        // Encode
                        let encoded_response =
                            or_next!("failed to serialize response", bin.serialize::<DaemonResponse>(&response));
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

    if let Err(err) = fs::remove_file(socket) {
        error!("failed to remove socket: {}", err);
    }
}
