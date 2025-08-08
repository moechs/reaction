use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::Arc,
};

use chrono::Local;
use futures::{SinkExt, StreamExt};
use regex::Regex;
use tokio::{fs, net::UnixListener};
use tokio_util::{
    bytes::Bytes,
    codec::{Framed, LengthDelimitedCodec},
};
use tracing::{error, warn};

use crate::{
    concepts::{Config, Filter, Pattern, Stream},
    protocol::{ClientRequest, ClientStatus, DaemonResponse, Order},
};

use super::{filter::FilterManager, shutdown::ShutdownToken};

async fn open_socket(path: PathBuf) -> Result<UnixListener, String> {
    macro_rules! err_str {
        ($expression:expr) => {
            $expression.map_err(|err| err.to_string())
        };
    }
    // First create all directories to the file
    let dir = path
        .parent()
        .ok_or(format!("socket {path:?} has no parent directory"))?;
    err_str!(fs::create_dir_all(dir).await)?;
    // Test if file exists
    match fs::metadata(&path).await {
        Ok(meta) => {
            if meta.file_type().is_dir() {
                Err(format!("socket {path:?} is already a directory"))
            } else {
                warn!("socket {path:?} already exists: is the daemon already running? deleting.");
                err_str!(fs::remove_file(&path).await)
            }
        }
        Err(err) => err_str!(match err.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(err),
        }),
    }?;
    // Open socket
    err_str!(UnixListener::bind(path))
}

fn handle_trigger_order(
    stream_name: Option<String>,
    filter_name: Option<String>,
    patterns: BTreeMap<Arc<Pattern>, String>,
    shared_state: &HashMap<&'static Stream, HashMap<&'static Filter, FilterManager>>,
) -> DaemonResponse {
    // Check names existence
    let (stream_name, filter_name) = match (stream_name, filter_name) {
        (Some(s), Some(p)) => (s, p),
        _ => {
            return DaemonResponse::Err(
                "trigger must target a filter, e.g. `reaction trigger mystream.myfilter ...`"
                    .into(),
            );
        }
    };

    // Check patterns existence
    if patterns.is_empty() {
        return DaemonResponse::Err(
            "trigger must specify patterns, e.g. `reaction trigger ... ip=1.2.3.4`".into(),
        );
    }

    // Check stream existance
    let filters = match shared_state
        .iter()
        .find(|(stream, _)| stream_name == stream.name)
    {
        Some((_, filters)) => filters,
        None => {
            return DaemonResponse::Err(format!("stream {stream_name} doesn't exist"));
        }
    };

    // Check filter existance
    let filter_manager = match filters
        .iter()
        .find(|(filter, _)| filter_name == filter.name)
    {
        Some((_, filter)) => filter,
        None => {
            return DaemonResponse::Err(format!(
                "filter {stream_name}.{filter_name} doesn't exist"
            ));
        }
    };

    let now = Local::now();
    match filter_manager.handle_trigger(patterns, now) {
        Ok(()) => DaemonResponse::Ok(()),
        Err(err) => DaemonResponse::Err(err),
    }
}

fn handle_show_or_flush_order(
    stream_name: Option<String>,
    filter_name: Option<String>,
    patterns: BTreeMap<Arc<Pattern>, Regex>,
    order: Order,
    shared_state: &HashMap<&'static Stream, HashMap<&'static Filter, FilterManager>>,
) -> DaemonResponse {
    let now = Local::now();
    let cs: ClientStatus = shared_state
        .iter()
        // stream filtering
        .filter(|(stream, _)| {
            stream_name.is_none() || stream_name.clone().is_some_and(|name| name == stream.name)
        })
        .fold(BTreeMap::new(), |mut acc, (stream, filter_manager)| {
            let inner_map = filter_manager
                .iter()
                // filter filtering
                .filter(|(filter, _)| {
                    filter_name.is_none()
                        || filter_name.clone().is_some_and(|name| name == filter.name)
                })
                // pattern filtering
                .filter(|(filter, _)| {
                    patterns
                        .iter()
                        .all(|(pattern, _)| filter.patterns.get(pattern).is_some())
                })
                .map(|(filter, manager)| {
                    (
                        filter.name.to_owned(),
                        manager.handle_order(&patterns, order, now),
                    )
                })
                .collect();
            acc.insert(stream.name.to_owned(), inner_map);
            acc
        });
    DaemonResponse::Order(cs)
}

fn answer_order(
    config: &'static Config,
    shared_state: &HashMap<&'static Stream, HashMap<&'static Filter, FilterManager>>,
    options: ClientRequest,
) -> DaemonResponse {
    // Compute options
    let (stream_name, filter_name) = match options.stream_filter {
        Some(sf) => match sf.split_once(".") {
            Some((s, f)) => (Some(s.to_string()), Some(f.to_string())),
            None => (Some(sf), None),
        },
        None => (None, None),
    };

    // Compute the Vec<(pattern_name: String, regex: String)> into a BTreeMap<Arc<Pattern>, String>
    let patterns = match options
        .patterns
        .into_iter()
        .map(|(name, reg)| {
            // lookup pattern in config.patterns
            config
                .patterns
                .iter()
                // retrieve or Err
                .find(|(pattern_name, _)| &name == *pattern_name)
                .ok_or_else(|| format!("pattern '{name}' doesn't exist"))
                .map(|(_, pattern)| (pattern.clone(), reg))
        })
        .collect::<Result<BTreeMap<Arc<Pattern>, String>, String>>()
    {
        Ok(p) => p,
        Err(err) => return DaemonResponse::Err(err),
    };

    if let Order::Trigger = options.order {
        handle_trigger_order(stream_name, filter_name, patterns, shared_state)
    } else {
        let patterns = match patterns
            .into_iter()
            .map(|(pattern, reg)| match Regex::new(&reg) {
                Ok(reg) => Ok((pattern, reg)),
                Err(err) => Err(format!(
                    "pattern '{}' regex doesn't compile: {err}",
                    pattern.name
                )),
            })
            .collect::<Result<BTreeMap<Arc<Pattern>, Regex>, String>>()
        {
            Ok(p) => p,
            Err(err) => return DaemonResponse::Err(err),
        };

        handle_show_or_flush_order(
            stream_name,
            filter_name,
            patterns,
            options.order,
            shared_state,
        )
    }
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

pub struct Socket {
    path: PathBuf,
    socket: UnixListener,
}

impl Socket {
    pub async fn open(socket: PathBuf) -> Result<Self, String> {
        Ok(Socket {
            socket: open_socket(socket.clone())
                .await
                .map_err(|err| format!("while creating communication socket: {err}"))?,
            path: socket,
        })
    }

    pub fn manager(
        self,
        config: &'static Config,
        shared_state: HashMap<&'static Stream, HashMap<&'static Filter, FilterManager>>,
        shutdown: ShutdownToken,
    ) {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.wait() => break,
                    try_conn = self.socket.accept() => {
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
                                    serde_json::from_slice(&encoded_request)
                                    );
                                // Process
                                let response = answer_order(config, &shared_state, request);
                                // Encode
                                let encoded_response =
                                    or_next!("failed to serialize response", serde_json::to_string::<DaemonResponse>(&response));
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

            if let Err(err) = fs::remove_file(self.path).await {
                error!("failed to remove socket: {}", err);
            }
        });
    }
}
