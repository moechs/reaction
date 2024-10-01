use std::{error::Error, io::stdout, os::unix::net::UnixStream, path::PathBuf, process::exit};

use bincode::Options;
use log::{debug, error, Level};

use crate::{
    concepts::{ClientRequest, ClientStatus, DaemonResponse, Order},
    utils::{bincode_options, cli::Format, SimpleLogger},
};
macro_rules! or_quit {
    ($msg:expr, $expression:expr) => {
        match $expression {
            Ok(x) => x,
            Err(err) => {
                error!("failed to communicate to daemon: {}, {}", $msg, err);
                exit(1);
            }
        }
    };
}

fn send_retrieve(socket: &PathBuf, req: &ClientRequest) -> DaemonResponse {
    let bin = bincode_options();
    let conn = or_quit!("opening connection to daemon", UnixStream::connect(socket));
    let conn2 = or_quit!("failed to clone stream", conn.try_clone());
    or_quit!("failed to send request", bin.serialize_into(conn, req));
    or_quit!(
        "failed to send request",
        bin.deserialize_from::<UnixStream, DaemonResponse>(conn2)
    )
}

fn print_status(cs: ClientStatus, format: Format) -> Result<(), Box<dyn Error>> {
    Ok(match format {
        Format::JSON => serde_json::to_writer(stdout().lock(), &cs)?,
        Format::YAML => serde_yaml::to_writer(stdout().lock(), &cs)?,
    })
}

pub fn request(
    socket: PathBuf,
    format: Format,
    stream_filter: Option<String>,
    patterns: Vec<(String, String)>,
    order: Order,
) {
    if let Err(err) = SimpleLogger::init(Level::Debug) {
        eprintln!("ERROR could not initialize logging: {err}");
        exit(1);
    }

    let response = send_retrieve(
        &socket,
        &ClientRequest {
            order,
            stream_filter,
            patterns,
        },
    );
    match response {
        DaemonResponse::Order(cs) => {
            if let Err(err) = print_status(cs, format) {
                error!("while printing response: {err}");
                exit(1);
            }
        }
        DaemonResponse::Err(err) => {
            error!("failed to communicate to daemon: error response: {err}");
            exit(1);
        }
    }
}

pub fn test_regex(config_path: PathBuf, regex: String, line: Option<String>) {
    if let Err(err) = SimpleLogger::init(Level::Debug) {
        eprintln!("ERROR could not initialize logging: {err}");
        exit(1);
    }
    debug!("test-regex {:?} {:?} {:?} ", config_path, regex, line);
}
