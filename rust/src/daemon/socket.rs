use std::{
    collections::BTreeMap,
    fs, io,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::exit,
    sync::{
        mpsc::{sync_channel, Sender},
        Arc,
    },
};

use bincode::Options;
use log::{error, warn};
use regex::Regex;

use crate::{
    concepts::{
        ActionFilter, ClientRequest, ClientStatus, Config, DaemonResponse, Pattern, PatternStatus,
    },
    utils::bincode_options,
};

use super::{statemap::FilterOptions, ExecsManagerInput, MatchManagerInput};

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

fn answer_order(
    config: &'static Config,
    match_tx: &Sender<MatchManagerInput>,
    exec_tx: &Sender<ExecsManagerInput>,
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
    let matches = {
        let (m_tx, m_rx) = sync_channel(0);

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        match_tx
            .send(MatchManagerInput::Order(
                options.order,
                filtering_options.clone(),
                m_tx,
            ))
            .unwrap();

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        m_rx.recv().unwrap()
    };

    // ask for execs clone
    let execs = {
        let (e_tx, e_rx) = sync_channel(0);

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        exec_tx
            .send(ExecsManagerInput::Order(
                options.order,
                filtering_options,
                e_tx,
            ))
            .unwrap();

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        e_rx.recv().unwrap()
    };

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

pub fn socket_manager(
    config: &'static Config,
    socket: PathBuf,
    match_tx: Sender<MatchManagerInput>,
    exec_tx: Sender<ExecsManagerInput>,
) {
    let listener = match open_socket(socket) {
        Ok(l) => l,
        Err(err) => {
            error!("while creating communication socket: {err}");
            exit(1);
        }
    };

    let bin = bincode_options();
    for try_conn in listener.incoming() {
        match try_conn {
            Ok(conn) => {
                let conn2 = or_next!("failed to clone stream", conn.try_clone());
                // read request
                let request = or_next!(
                    "invalid message received: ",
                    bin.deserialize_from::<UnixStream, ClientRequest>(conn)
                );
                let response = answer_order(config, &match_tx, &exec_tx, request);
                or_next!(
                    "failed to send response:",
                    match response {
                        Ok(res) => bin.serialize_into(conn2, &DaemonResponse::Order(res)),
                        Err(err) => bin.serialize_into(conn2, &DaemonResponse::Err(err)),
                    }
                );
            }
            Err(err) => error!("failed to open connection from cli: {err}"),
        }
    }
}
