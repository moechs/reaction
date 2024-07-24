use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{path::PathBuf, sync::Arc};

use log::{debug, error, warn, Level};

use crate::{config, logger};

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

    // This AtomicBool prevents us from freeing resources two times
    let quitting = Arc::new(AtomicBool::new(false));
    {
        // Handle SIGINT, SIGTERM, SIGHUP
        let quitting_ctrlc = quitting.clone();
        let config_ctrlc = config.clone();
        if let Err(err) = ctrlc::set_handler(move || quit(&quitting_ctrlc, &config_ctrlc, false)) {
            error!("impossible to launch a thread for catching signals, exiting: {err}");
            exit(1);
        }
    }

    if !&config.start() {
        error!("a start command failed, exiting.");
        exit(1);
    }

    // TODO launch streams

    // TODO wait for streams to finish

    quit(&quitting, &config, true);
}

fn quit(quitting: &Arc<AtomicBool>, _config: &config::Config, graceful: bool) {
    if let Err(_) = quitting.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst) {
        warn!("tried to quit while already quitting");
        return;
    }

    // TODO free resources, flush DB etc.

    exit(match graceful {
        true => 0,
        false => 1,
    });
}
