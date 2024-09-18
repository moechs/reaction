use std::{
    path::PathBuf,
    process::exit,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, sync_channel},
        Arc,
    },
    thread,
};

use log::{debug, error, info, Level};

use crate::{
    config::Config,
    database::database_manager,
    execs::{execs_manager, ExecsManagerInput},
    matches::{matches_manager, MatchManagerInput},
    utils::SimpleLogger,
};

pub fn daemon(config_path: &PathBuf, loglevel: Level, socket: &PathBuf) {
    if let Err(err) = SimpleLogger::init(loglevel) {
        eprintln!("ERROR could not initialize logging: {err}");
        exit(1);
    }

    debug!("daemon {config_path:?} {loglevel:?} {socket:?}");

    let config = match Config::from_file(config_path) {
        Ok(config) => Arc::new(config),
        Err(err) => {
            error!("{err}");
            exit(1);
        }
    };

    debug!("config: {:?}", config);

    if !config.start() {
        error!("a start command failed, exiting.");
        exit(1);
    }

    let mut stream_process_child_handles = Vec::new();
    let mut stream_thread_handles = Vec::new();

    let (match_tx, match_rx) = channel();
    let (exec_tx, exec_rx) = channel();
    let (log_tx, log_rx) = channel();

    let matches_manager_thread_handle = {
        let config_matches = config.clone();
        let match_tx_matches = match_tx.clone();
        let exec_tx_matches = exec_tx.clone();
        thread::spawn(move || {
            matches_manager(
                config_matches,
                match_rx,
                match_tx_matches,
                exec_tx_matches,
                log_tx,
            )
        })
    };

    let execs_manager_thread_handle = {
        let config_execs = config.clone();
        let exec_tx_execs = exec_tx.clone();
        thread::spawn(move || execs_manager(config_execs, exec_rx, exec_tx_execs))
    };

    let database_manager_thread_handle = {
        let config_database = config.clone();
        let match_tx_database = match_tx.clone();
        // The `thread::spawn` is done in the function, after database rotation is finished
        database_manager(config_database, log_rx, match_tx_database)
    };

    for stream in config.streams().values() {
        let stream = stream.clone();
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

    match_tx.send(MatchManagerInput::Stop).unwrap();
    let _ = matches_manager_thread_handle.join();

    exec_tx.send(ExecsManagerInput::Stop).unwrap();
    let _ = execs_manager_thread_handle.join();

    let _ = database_manager_thread_handle.join();

    let stop_ok = config.stop();

    // TODO flush DB

    // TODO remove socket

    exit(match !signal_received.load(Ordering::SeqCst) && stop_ok {
        true => 0,
        false => 1,
    });
}
