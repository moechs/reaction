use std::path::PathBuf;

use crate::cli::LogLevel;

pub fn daemon(config_path: &PathBuf, loglevel: LogLevel, socket: &PathBuf) {
    println!("daemon {:?} {:?} {:?}", config_path, loglevel, socket);
}
