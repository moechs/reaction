use std::{
    error::Error,
    path::{Path, PathBuf},
};

use bincode::Options;
use futures::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio_util::{
    bytes::Bytes,
    codec::{Framed, LengthDelimitedCodec},
};

use crate::{
    cli::Format,
    protocol::{bincode_options, ClientRequest, ClientStatus, DaemonResponse, Order},
};

macro_rules! or_quit {
    ($msg:expr, $expression:expr) => {
        $expression.map_err(|err| format!("failed to communicate to daemon: {}, {}", $msg, err))?
    };
}

async fn send_retrieve(socket: &Path, req: &ClientRequest) -> Result<DaemonResponse, String> {
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
    Ok(or_quit!(
        "failed to decode response",
        bin.deserialize::<DaemonResponse>(&encoded_response)
    ))
}

fn print_status(cs: ClientStatus, format: Format) -> Result<(), Box<dyn Error>> {
    let encoded = match format {
        Format::JSON => serde_json::to_string_pretty(&cs)?,
        Format::YAML => serde_yaml::to_string(&cs)?,
    };
    println!("{}", encoded);
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
        DaemonResponse::Order(cs) => {
            print_status(cs, format).map_err(|err| format!("while printing response: {err}"))
        }
        DaemonResponse::Err(err) => Err(format!(
            "failed to communicate to daemon: error response: {err}"
        )),
    }?;
    Ok(())
}
