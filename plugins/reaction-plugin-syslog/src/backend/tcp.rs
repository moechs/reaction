//! TCP syslog listener.

use std::{sync::Arc, time::Duration};

use reaction_plugin::shutdown::{ShutdownController, ShutdownToken};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
    sync::Semaphore,
};

use crate::{dispatch::Dispatcher, lines::TcpFramer};

pub async fn serve(
    listener: TcpListener,
    dispatcher: Dispatcher,
    shutdown: ShutdownController,
    listener_token: ShutdownToken,
) {
    let connections = Arc::new(Semaphore::new(dispatcher.max_tcp_connections()));
    let idle_timeout = dispatcher.tcp_idle_timeout();
    loop {
        let accepted = tokio::select! {
            _ = listener_token.wait() => return,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((socket, peer)) => {
                let permit = match Arc::clone(&connections).try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        log_warn_throttled!(
                            dispatcher.warn_throttle(),
                            "tcp_connection_limit",
                            "TCP connection limit reached, dropping connection from {peer}"
                        );
                        continue;
                    }
                };
                let token = shutdown.token();
                let dispatcher = dispatcher.clone();
                let peer = Arc::new(peer.to_string());
                tokio::spawn(async move {
                    let _permit = permit;
                    handle_connection(socket, dispatcher, token, peer, idle_timeout).await;
                });
            }
            Err(err) => {
                log_warn_throttled!(
                    dispatcher.warn_throttle(),
                    "tcp_accept",
                    "TCP accept failed: {err}"
                );
                if super::sleep_or_shutdown(dispatcher.reconnect_delay(), listener_token.wait())
                    .await
                {
                    return;
                }
            }
        }
    }
}

async fn handle_connection(
    mut socket: TcpStream,
    dispatcher: Dispatcher,
    token: ShutdownToken,
    peer: Arc<String>,
    idle_timeout: Duration,
) {
    let mut framer = TcpFramer::new(
        dispatcher.max_line_bytes(),
        dispatcher.framing(),
        dispatcher.warn_throttle(),
    );
    let mut buf = [0u8; 8192];
    let mut lines = Vec::new();
    loop {
        let read = tokio::select! {
            _ = token.wait() => return,
            read = tokio::time::timeout(idle_timeout, socket.read(&mut buf)) => match read {
                Ok(read) => read,
                Err(_) => {
                    log_warn_throttled!(
                        dispatcher.warn_throttle(),
                        "tcp_idle",
                        "TCP connection from {peer} timed out"
                    );
                    return;
                }
            },
        };
        match read {
            Ok(0) => {
                if let Some(line) = framer.flush() {
                    dispatcher.dispatch_tcp_frame(&line, Some(peer.to_string()));
                }
                return;
            }
            Ok(len) => {
                lines.clear();
                framer.push(&buf[..len], &mut lines);
                for line in lines.drain(..) {
                    dispatcher.dispatch_tcp_frame(&line, Some(peer.to_string()));
                }
            }
            Err(err) => {
                log_warn_throttled!(
                    dispatcher.warn_throttle(),
                    "tcp_connection",
                    "TCP connection from {peer} failed: {err}"
                );
                return;
            }
        }
    }
}
