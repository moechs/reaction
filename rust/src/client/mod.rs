use std::{
    collections::BTreeSet,
    error::Error,
    io::{stdin, stdout, BufRead, BufReader},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::exit,
    sync::Arc,
};

use bincode::Options;
use log::{error, info, Level};
use regex::Regex;

use crate::{
    concepts::{ClientRequest, ClientStatus, Config, DaemonResponse, Order, Pattern},
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

pub fn test_regex(config_path: PathBuf, mut regex: String, line: Option<String>) {
    if let Err(err) = SimpleLogger::init(Level::Debug) {
        eprintln!("ERROR could not initialize logging: {err}");
        exit(1);
    }

    let config: Config = match Config::from_file(&config_path) {
        Ok(config) => config,
        Err(err) => {
            error!("{err}");
            exit(1);
        }
    };

    // Code close to Filter::setup()
    let mut used_patterns: BTreeSet<Arc<Pattern>> = BTreeSet::new();
    for pattern in config.patterns().values() {
        if let Some(index) = regex.find(pattern.name_with_braces()) {
            // we already `find` it, so we must be able to `rfind` it
            #[allow(clippy::unwrap_used)]
            if regex.rfind(pattern.name_with_braces()).unwrap() != index {
                error!(
                    "pattern {} present multiple times in regex",
                    pattern.name_with_braces()
                );
                exit(1);
            }
            used_patterns.insert(pattern.clone());
        }
        regex = regex.replacen(pattern.name_with_braces(), &pattern.regex, 1);
    }

    let compiled = match Regex::new(&regex) {
        Ok(reg) => reg,
        Err(err) => {
            error!("regex doesn't compile: {err}");
            exit(1);
        }
    };

    let match_closure = |line: String| {
        let mut ignored = false;
        if let Some(matches) = compiled.captures(&line) {
            let mut result = Vec::new();
            if used_patterns.len() != 0 {
                for pattern in used_patterns.iter() {
                    if let Some(match_) = matches.name(pattern.name()) {
                        result.push(match_.as_str().to_string());
                        if !pattern.not_an_ignore(match_.as_str()) {
                            ignored = true;
                        }
                    }
                }
                if !ignored {
                    println!("\x1b[32mmatching\x1b[0m {result:?}: {line}");
                } else {
                    println!("\x1b[33mignore matching\x1b[0m {result:?}: {line}");
                }
            } else {
                println!("\x1b[32mmatching\x1b[0m: {line}");
            }
        } else {
            println!("\x1b[31mno match\x1b[0m: {line}");
        }
    };

    if let Some(line) = line {
        match_closure(line);
    } else {
        info!("no second argument: reading from stdin");
        for line in BufReader::new(stdin()).lines() {
            match line {
                Ok(line) => match_closure(line),
                Err(_) => break,
            };
        }
    }
}
