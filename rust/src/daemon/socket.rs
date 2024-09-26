use std::{
    fs, io,
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::exit,
    sync::mpsc::{sync_channel, Sender},
};

use bincode::Options;
use chrono::Local;
use log::{error, warn};

use crate::{
    concepts::{ClientRequest, Config, DaemonResponse, InfoRes, MFT},
    utils::bincode_options,
};

use super::{ExecsManagerInput, MatchManagerInput};

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
                let response = match request {
                    ClientRequest::Info => {
                        // ask for matches clone
                        let (m_tx, m_rx) = sync_channel(0);
                        #[allow(clippy::unwrap_used)] // propagating panics is ok
                        match_tx.send(MatchManagerInput::Gimme(m_tx)).unwrap();
                        #[allow(clippy::unwrap_used)] // propagating panics is ok
                        let matches = m_rx.recv().unwrap();

                        // ask for execs clone
                        let (e_tx, e_rx) = sync_channel(0);
                        #[allow(clippy::unwrap_used)] // propagating panics is ok
                        exec_tx.send(ExecsManagerInput::Gimme(e_tx)).unwrap();
                        #[allow(clippy::unwrap_used)] // propagating panics is ok
                        let execs = e_rx.recv().unwrap();

                        // Transform structures
                        macro_rules! map_map {
                            ($map:expr) => {
                                $map.into_iter()
                                    .map(|(object, inner_map)| {
                                        (
                                            object.full_name(),
                                            inner_map
                                                .into_iter()
                                                .map(|(key, set)| {
                                                    (
                                                        key,
                                                        set.into_iter()
                                                            .map(|time| time.timestamp())
                                                            .collect(),
                                                    )
                                                })
                                                .collect(),
                                        )
                                    })
                                    .collect()
                            };
                        }

                        DaemonResponse::Info(InfoRes {
                            matches: map_map!(matches),
                            execs: map_map!(execs),
                        })
                    }
                    ClientRequest::Flush(flush) => {
                        match config.get_filter(&flush.f) {
                            Some(filter) => {
                                let now = Local::now();
                                // Flush actions
                                filter.send_actions(&flush.m, now, &exec_tx, false);
                                // Flush filters
                                #[allow(clippy::unwrap_used)] // propagating panics is ok
                                match_tx
                                    .send(MatchManagerInput::Flush(MFT {
                                        m: flush.m,
                                        f: filter,
                                        t: now,
                                    }))
                                    .unwrap();
                                DaemonResponse::Flush
                            }
                            None => DaemonResponse::Err(format!(
                                "no filter with name {}.{}",
                                flush.f.0, flush.f.1
                            )),
                        }
                    }
                };
                or_next!(
                    "failed to send response:",
                    bin.serialize_into(conn2, &response)
                );
            }
            Err(err) => error!("failed to open connection from cli: {err}"),
        }
    }
}
