//! UDP syslog listener.

use reaction_plugin::shutdown::ShutdownToken;
use tokio::net::UdpSocket;

use crate::dispatch::Dispatcher;

pub async fn serve(socket: UdpSocket, dispatcher: Dispatcher, token: ShutdownToken) {
    let mut buf = vec![0u8; dispatcher.max_datagram_bytes() + 1];
    loop {
        let received = tokio::select! {
            _ = token.wait() => return,
            received = socket.recv_from(&mut buf) => received,
        };
        match received {
            Ok((len, peer)) => {
                dispatcher.dispatch_bytes(&buf[..len], Some(peer.to_string()));
            }
            Err(err) => {
                log_warn_throttled!(
                    dispatcher.warn_throttle(),
                    "udp_receive",
                    "UDP receive failed: {err}"
                );
                if super::sleep_or_shutdown(dispatcher.reconnect_delay(), token.wait()).await {
                    return;
                }
            }
        }
    }
}
