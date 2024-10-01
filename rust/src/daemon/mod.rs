use std::{
    fs,
    path::PathBuf,
    process::exit,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, sync_channel},
        Arc,
    },
    thread,
};

use log::{error, info, Level};
use socket::socket_manager;

use crate::{concepts::Config, utils::SimpleLogger};
use database::database_manager;
use execs::execs_manager;
use matches::matches_manager;

pub use execs::ExecsManagerInput;
pub use matches::MatchManagerInput;

mod database;
mod execs;
mod matches;
mod socket;
mod statemap;

#[allow(unused_variables)]
pub fn daemon(config_path: PathBuf, loglevel: Level, socket: PathBuf) {
    if let Err(err) = SimpleLogger::init(loglevel) {
        eprintln!("ERROR could not initialize logging: {err}");
        exit(1);
    }

    let config: &'static Config = match Config::from_file(&config_path) {
        Ok(config) => Box::leak(Box::new(config)),
        Err(err) => {
            error!("{err}");
            exit(1);
        }
    };

    if !config.start() {
        error!("a start command failed, exiting.");
        exit(1);
    }

    let mut stream_process_child_handles = Vec::new();
    let mut stream_thread_handles = Vec::new();

    // FIXME replace those channel() by sync_channel(1)
    // Unbounded channels can eat a lot of RAM
    // Is there a timer::MessageTimer implementation for sync_channel?
    let (match_tx, match_rx) = channel();
    let (exec_tx, exec_rx) = channel();
    let (log_tx, log_rx) = sync_channel(1);

    let matches_manager_thread_handle = {
        let match_tx_matches = match_tx.clone();
        let exec_tx_matches = exec_tx.clone();
        let log_tx_matches = log_tx.clone();
        thread::spawn(move || {
            matches_manager(match_rx, match_tx_matches, exec_tx_matches, log_tx_matches)
        })
    };

    let execs_manager_thread_handle = {
        let exec_tx_execs = exec_tx.clone();
        thread::spawn(move || execs_manager(config, exec_rx, exec_tx_execs, log_tx))
    };

    let database_manager_thread_handle = {
        let match_tx_database = match_tx.clone();
        // The `thread::spawn` is done in the function, after database rotation is finished
        database_manager(config, log_rx, match_tx_database)
    };

    {
        let match_tx_socket = match_tx.clone();
        let exec_tx_socket = exec_tx.clone();
        let socket = socket.to_owned();
        thread::spawn(move || socket_manager(config, socket, match_tx_socket, exec_tx_socket));
    }

    for stream in config.streams().values() {
        let match_tx = match_tx.clone();
        let (child_tx, child_rx) = sync_channel(0);

        stream_thread_handles.push(thread::spawn(move || stream.manager(child_tx, match_tx)));

        if let Ok(Some(child)) = child_rx.recv() {
            stream_process_child_handles.push(child);
        }
    }

    let signal_received = Arc::new(AtomicBool::new(false));
    {
        // Handle SIGINT, SIGTERM, SIGHUP
        let signal_received2 = signal_received.clone();
        if let Err(err) = ctrlc::set_handler(move || {
            signal_received2.store(true, Ordering::SeqCst);

            info!("waiting for streams to finish...");
            // Kill stream subprocesses
            for child_handle in stream_process_child_handles.iter_mut() {
                let _ = child_handle.kill();
            }
        }) {
            error!("impossible to launch a signal-catching thread, exiting: {err}");
            exit(1);
        }
    }

    // Wait for all streams to quit
    for thread_handle in stream_thread_handles {
        let _ = thread_handle.join();
    }

    #[allow(clippy::unwrap_used)] // propagating panics is ok
    match_tx.send(MatchManagerInput::Stop).unwrap();
    let _ = matches_manager_thread_handle.join();

    #[allow(clippy::unwrap_used)] // propagating panics is ok
    exec_tx.send(ExecsManagerInput::Stop).unwrap();
    let _ = execs_manager_thread_handle.join();

    let _ = database_manager_thread_handle.join();

    let stop_ok = config.stop();

    // not waiting for the socket_manager to finish, sorry
    if let Err(err) = fs::remove_file(socket) {
        error!("failed to remove socket: {}", err);
    }

    exit(match !signal_received.load(Ordering::SeqCst) && stop_ok {
        true => 0,
        false => 1,
    });
}
