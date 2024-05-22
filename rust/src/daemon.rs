use std::path::PathBuf;

use crate::{cli::LogLevel, config};

pub fn daemon(config_path: &PathBuf, loglevel: LogLevel, socket: &PathBuf) {
    println!("daemon {:?} {:?} {:?}", config_path, loglevel, socket);
    let config = config::config_from_file(config_path);
    if let Ok(config) = config {
        dbg!(config);
    } else if let Err(err) = config {
        dbg!(err);
    }
}
