use std::{
    collections::BTreeSet,
    error::Error,
    io::{stdin, BufRead, BufReader},
    path::PathBuf,
    sync::Arc,
};

use bincode::Options;
use futures::{SinkExt, StreamExt};
use log::info;
use regex::Regex;
use tokio::{io::AsyncWriteExt, net::UnixStream};
use tokio_util::{
    bytes::Bytes,
    codec::{Framed, LengthDelimitedCodec},
};

use crate::{
    concepts::{ClientRequest, ClientStatus, Config, DaemonResponse, Order, Pattern},
    utils::{bincode_options, cli::Format},
};

macro_rules! or_quit {
    ($msg:expr, $expression:expr) => {
        $expression.map_err(|err| format!("failed to communicate to daemon: {}, {}", $msg, err))?
    };
}

async fn send_retrieve(socket: &PathBuf, req: &ClientRequest) -> Result<DaemonResponse, String> {
    let bin = bincode_options();
    let conn = or_quit!(
        "opening connection to daemon",
        UnixStream::connect(socket).await
    );
    // Encode
    let mut transport = Framed::new(conn, LengthDelimitedCodec::new());
    let encoded_request = or_quit!("failed to encode request", bin.serialize(req));
    or_quit!(
        "failed to send request",
        transport.send(Bytes::from(encoded_request)).await
    );
    // Decode
    let encoded_response = or_quit!(
        "failed to read response",
        transport.next().await.ok_or("empty response from server")
    );
    let encoded_response = or_quit!("failed to decode response", encoded_response);
    or_quit!(
        "failed to decode response",
        bin.deserialize(&encoded_response)
    )
}

async fn print_status(cs: ClientStatus, format: Format) -> Result<(), Box<dyn Error>> {
    let encoded = match format {
        Format::JSON => serde_json::to_string_pretty(&cs)?,
        Format::YAML => serde_yaml::to_string(&cs)?,
    };
    tokio::io::stdout().write_all(encoded.as_bytes()).await?;
    Ok(())
}

pub async fn request(
    socket: PathBuf,
    format: Format,
    stream_filter: Option<String>,
    patterns: Vec<(String, String)>,
    order: Order,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let response = send_retrieve(
        &socket,
        &ClientRequest {
            order,
            stream_filter,
            patterns,
        },
    )
    .await;
    match response? {
        DaemonResponse::Order(cs) => print_status(cs, format)
            .await
            .map_err(|err| format!("while printing response: {err}")),
        DaemonResponse::Err(err) => Err(format!(
            "failed to communicate to daemon: error response: {err}"
        )),
    }?;
    Ok(())
}

pub fn test_regex(
    config_path: PathBuf,
    mut regex: String,
    line: Option<String>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let config: Config = Config::from_file(&config_path)?;

    // Code close to Filter::setup()
    let mut used_patterns: BTreeSet<Arc<Pattern>> = BTreeSet::new();
    for pattern in config.patterns().values() {
        if let Some(index) = regex.find(pattern.name_with_braces()) {
            // we already `find` it, so we must be able to `rfind` it
            #[allow(clippy::unwrap_used)]
            if regex.rfind(pattern.name_with_braces()).unwrap() != index {
                return Err(format!(
                    "pattern {} present multiple times in regex",
                    pattern.name_with_braces()
                )
                .into());
            }
            used_patterns.insert(pattern.clone());
        }
        regex = regex.replacen(pattern.name_with_braces(), &pattern.regex, 1);
    }

    let compiled = Regex::new(&regex).map_err(|err| format!("regex doesn't compile: {err}"))?;

    let match_closure = |line: String| {
        let mut ignored = false;
        if let Some(matches) = compiled.captures(&line) {
            let mut result = Vec::new();
            if !used_patterns.is_empty() {
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
    Ok(())
}
