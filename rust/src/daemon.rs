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

use crate::{config, logger, matches::matches_manager};

pub fn daemon(config_path: &PathBuf, loglevel: Level, socket: &PathBuf) {
    if let Err(err) = logger::SimpleLogger::init(loglevel) {
        eprintln!("ERROR could not initialize logging: {err}");
        exit(1);
    }

    debug!("daemon {config_path:?} {loglevel:?} {socket:?}");

    let config = match config::config_from_file(config_path) {
        Ok(config) => Arc::new(config),
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

    {
        let (match_tx, match_rx) = channel();
        let (action_tx, _action_rx) = channel();

        let config_matches = config.clone();
        stream_thread_handles.push(thread::spawn(move || {
            matches_manager(config_matches, match_rx, action_tx)
        }));

        // TODO execs manager

        for stream in config.streams().values() {
            let stream = stream.clone();
            let match_tx = match_tx.clone();
            let (child_tx, child_rx) = sync_channel(0);

            stream_thread_handles.push(thread::spawn(move || stream.manager(child_tx, match_tx)));

            if let Ok(Some(child)) = child_rx.recv() {
                stream_process_child_handles.push(child);
            }
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

    // TODO wait for actions to complete

    let stop_ok = config.stop();

    // TODO flush DB

    // TODO remove socket

    exit(match !signal_received.load(Ordering::SeqCst) && stop_ok {
        true => 0,
        false => 1,
    });
}
