use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicU16, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reaction_plugin::{ActionConfig, PluginInfo, StreamConfig, Value};
use serde_json::{Value as JsonValue, json};
use tokio::io::AsyncWriteExt;

use crate::{
    config::{PluginOptions, StreamOptions},
    dispatch::Dispatcher,
    plugin::{Plugin, STREAM_TYPE, Sink},
    template::Template,
};

fn stream(name: &str, stream_type: &str) -> StreamConfig {
    StreamConfig {
        stream_name: name.into(),
        stream_type: stream_type.into(),
        config: json!({}).into(),
    }
}

fn plugin_options() -> Vec<Value> {
    vec![json!({"listen": "127.0.0.1:5140"}).into()]
}

#[tokio::test]
async fn manifest_advertises_syslog_stream_only() {
    let manifest = Plugin::default().manifest().await.unwrap();
    assert_eq!(manifest.streams, BTreeSet::from([STREAM_TYPE.to_string()]));
    assert!(manifest.actions.is_empty());
}

#[tokio::test]
async fn load_config_creates_one_receiver_per_stream() {
    let mut plugin = Plugin::default();
    let (streams, actions) = plugin
        .load_config(
            vec![stream("auth", STREAM_TYPE), stream("mail", STREAM_TYPE)],
            vec![],
            plugin_options(),
        )
        .await
        .unwrap();
    assert_eq!(streams.len(), 2);
    assert!(streams.iter().all(|s| s.standalone));
    assert!(actions.is_empty());

    plugin.start().await.unwrap();
    plugin.close().await.unwrap();
}

#[tokio::test]
async fn load_config_rejects_actions() {
    let action = ActionConfig {
        stream_name: "auth".into(),
        filter_name: "failed".into(),
        action_name: "ban".into(),
        action_type: "syslog".into(),
        config: json!({}).into(),
        patterns: vec![],
    };
    let result = Plugin::default()
        .load_config(vec![], vec![action], plugin_options())
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn load_config_rejects_unknown_stream_type() {
    let result = Plugin::default()
        .load_config(vec![stream("auth", "docker")], vec![], plugin_options())
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn load_config_rejects_invalid_plugin_options() {
    let options = vec![json!({"listen": "invalid"}).into()];
    let result = Plugin::default()
        .load_config(vec![stream("auth", STREAM_TYPE)], vec![], options)
        .await;
    assert!(result.is_err());
}

static NEXT_PORT: AtomicU16 = AtomicU16::new(42000);

fn free_port() -> u16 {
    NEXT_PORT.fetch_add(1, Ordering::Relaxed)
}

fn merge_options(base: &mut JsonValue, extra: JsonValue) {
    if let Some(extra) = extra.as_object() {
        let target = base.as_object_mut().expect("options object");
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
}

async fn plugin_on_port_with_options(
    port: u16,
    extra: JsonValue,
) -> (Plugin, reaction_plugin::StreamImpl) {
    let mut plugin = Plugin::default();
    let mut options = json!({"listen": format!("127.0.0.1:{port}")});
    merge_options(&mut options, extra);
    let (streams, _) = plugin
        .load_config(
            vec![stream("auth", STREAM_TYPE)],
            vec![],
            vec![options.into()],
        )
        .await
        .unwrap();
    plugin.start().await.unwrap();
    (plugin, streams.into_iter().next().unwrap())
}

async fn plugin_on_port(port: u16, max_line_bytes: usize) -> (Plugin, reaction_plugin::StreamImpl) {
    plugin_on_port_with_options(port, json!({"max_line_bytes": max_line_bytes})).await
}

#[tokio::test]
async fn udp_datagram_becomes_sanitized_line() {
    let port = free_port();
    let (plugin, stream) = plugin_on_port(port, 1024).await;
    let mut receiver = stream.stream;
    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sender
        .send_to(b"hello\r\0", ("127.0.0.1", port))
        .await
        .unwrap();

    let line = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(line.0, "hello");
    plugin.close().await.unwrap();
}

#[tokio::test]
async fn oversized_udp_datagram_is_dropped() {
    let port = free_port();
    let (plugin, stream) = plugin_on_port(port, 8).await;
    let mut receiver = stream.stream;
    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sender
        .send_to(b"too-long-line", ("127.0.0.1", port))
        .await
        .unwrap();

    let result = tokio::time::timeout(std::time::Duration::from_millis(300), receiver.recv()).await;
    assert!(result.is_err());
    plugin.close().await.unwrap();
}

#[tokio::test]
async fn close_drops_receivers() {
    let port = free_port();
    let (plugin, stream) = plugin_on_port(port, 1024).await;
    let mut receiver = stream.stream;
    plugin.close().await.unwrap();

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn selects_streams_and_renders_prefix() {
    let port = free_port();
    let mut plugin = Plugin::default();
    let options = vec![json!({"listen": format!("127.0.0.1:{port}")}).into()];
    let streams = vec![
        StreamConfig {
            stream_name: "auth".into(),
            stream_type: STREAM_TYPE.into(),
            config: json!({
                "select": {"facilities": ["auth"], "severities": ["crit"]},
                "prefix": "{host} {appname}: "
            })
            .into(),
        },
        StreamConfig {
            stream_name: "mail".into(),
            stream_type: STREAM_TYPE.into(),
            config: json!({"select": {"facilities": ["mail"]}}).into(),
        },
    ];
    let (impls, _) = plugin.load_config(streams, vec![], options).await.unwrap();
    plugin.start().await.unwrap();
    let mut receivers: Vec<_> = impls.into_iter().map(|stream| stream.stream).collect();

    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sender
        .send_to(
            b"<34>Feb  5 17:32:18 host sshd[1234]: Failed password",
            ("127.0.0.1", port),
        )
        .await
        .unwrap();

    let line = tokio::time::timeout(std::time::Duration::from_secs(2), receivers[0].recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(line.0, "host sshd: Failed password");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), receivers[1].recv())
            .await
            .is_err()
    );
    plugin.close().await.unwrap();
}

async fn tcp_plugin_on_port_with_options(
    port: u16,
    extra: JsonValue,
) -> (Plugin, reaction_plugin::StreamImpl) {
    let mut options = json!({"protocol": "tcp"});
    merge_options(&mut options, extra);
    plugin_on_port_with_options(port, options).await
}

async fn tcp_plugin_on_port(port: u16) -> (Plugin, reaction_plugin::StreamImpl) {
    tcp_plugin_on_port_with_options(port, json!({})).await
}

async fn recv_line(receiver: &mut remoc::rch::mpsc::Receiver<reaction_plugin::Line>) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .0
}

#[tokio::test]
async fn tcp_newline_framing() {
    let port = free_port();
    let (plugin, stream) = tcp_plugin_on_port(port).await;
    let mut receiver = stream.stream;
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    client.write_all(b"one\ntwo\n").await.unwrap();

    assert_eq!(recv_line(&mut receiver).await, "one");
    assert_eq!(recv_line(&mut receiver).await, "two");
    plugin.close().await.unwrap();
}

#[tokio::test]
async fn tcp_octet_counted_framing() {
    let port = free_port();
    let (plugin, stream) = tcp_plugin_on_port(port).await;
    let mut receiver = stream.stream;
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    client.write_all(b"11 <165>1 host5 hello").await.unwrap();

    assert_eq!(recv_line(&mut receiver).await, "<165>1 host");
    assert_eq!(recv_line(&mut receiver).await, "hello");
    plugin.close().await.unwrap();
}

#[tokio::test]
async fn full_channel_drops_new_lines() {
    let (tx, mut rx) = remoc::rch::mpsc::channel(1);
    let sink = Sink {
        stream_name: "auth".into(),
        options: StreamOptions::default(),
        template: Template::default(),
        tx,
    };
    let options = PluginOptions::parse(vec![
        json!({
            "listen": "127.0.0.1:5140",
            "reconnect_delay": "100ms",
        })
        .into(),
    ])
    .unwrap();
    let dispatcher = Dispatcher::new(options, vec![sink]);

    dispatcher.dispatch_text("one".into(), None);
    dispatcher.dispatch_text("two".into(), None);

    assert_eq!(rx.try_recv().unwrap().0, "one");
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn close_stops_active_tcp_connections() {
    let port = free_port();
    let (plugin, _stream) = tcp_plugin_on_port(port).await;
    let _client = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(2), plugin.close())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn multibyte_datagram_does_not_stop_listener() {
    let port = free_port();
    let (plugin, stream) = plugin_on_port(port, 1024).await;
    let mut receiver = stream.stream;
    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();

    sender
        .send_to("<13>éééééééé".as_bytes(), ("127.0.0.1", port))
        .await
        .unwrap();
    assert_eq!(recv_line(&mut receiver).await, "<13>éééééééé");

    sender
        .send_to(b"still listening", ("127.0.0.1", port))
        .await
        .unwrap();
    assert_eq!(recv_line(&mut receiver).await, "still listening");
    plugin.close().await.unwrap();
}

#[tokio::test]
async fn tcp_lines_ignore_max_datagram_bytes() {
    let port = free_port();
    let (plugin, stream) = tcp_plugin_on_port_with_options(
        port,
        json!({"max_datagram_bytes": 8, "max_line_bytes": 1024}),
    )
    .await;
    let mut receiver = stream.stream;
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();

    client.write_all(b"1234567890\n").await.unwrap();
    assert_eq!(recv_line(&mut receiver).await, "1234567890");
    plugin.close().await.unwrap();
}

#[tokio::test]
async fn close_waits_for_tcp_listener() {
    let port = free_port();
    let (plugin, _stream) = tcp_plugin_on_port(port).await;

    plugin.close().await.unwrap();
    let rebound = tokio::net::TcpListener::bind(("127.0.0.1", port)).await;
    assert!(rebound.is_ok());
}

#[tokio::test]
async fn tcp_connection_limit_rejects_extra_clients() {
    let port = free_port();
    let (plugin, stream) =
        tcp_plugin_on_port_with_options(port, json!({"max_tcp_connections": 1})).await;
    let mut receiver = stream.stream;
    let _first = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let mut second = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();

    let _ = second.write_all(b"second\n").await;
    assert!(
        tokio::time::timeout(Duration::from_millis(300), receiver.recv())
            .await
            .is_err()
    );
    plugin.close().await.unwrap();
}

#[tokio::test]
async fn tcp_idle_timeout_drops_connection() {
    let port = free_port();
    let (plugin, stream) =
        tcp_plugin_on_port_with_options(port, json!({"tcp_idle_timeout": "100ms"})).await;
    let mut receiver = stream.stream;
    let mut client = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = client.write_all(b"late\n").await;
    assert!(
        tokio::time::timeout(Duration::from_millis(300), receiver.recv())
            .await
            .is_err()
    );
    plugin.close().await.unwrap();
}

#[tokio::test]
async fn out_of_range_message_timestamp_falls_back_to_receive_time() {
    let port = free_port();
    let (plugin, stream) = plugin_on_port_with_options(port, json!({"timestamp": "message"})).await;
    let mut receiver = stream.stream;
    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let before = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

    sender
        .send_to(
            b"<165>1 2099-01-01T00:00:00Z host app - - - future message",
            ("127.0.0.1", port),
        )
        .await
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let after = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

    assert_eq!(received.0, "future message");
    assert!(received.1 >= before && received.1 <= after);
    plugin.close().await.unwrap();
}
