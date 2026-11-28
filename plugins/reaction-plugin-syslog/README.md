# reaction-plugin-syslog

Syslog input plugin for reaction. It receives messages over UDP or TCP, parses the syslog envelope, and emits reaction lines to configured streams. It is a pure input plugin; filtering and actions stay in reaction core.

## Contract with reaction core

- `manifest()` advertises stream type `syslog` and no action types.
- `load_config()` validates options and creates one `remoc::rch::mpsc::channel` per stream. It does not bind sockets or start tasks.
- `start()` binds the listener and spawns receive tasks. A bind error aborts reaction startup.
- `close()` asks the shutdown controller to stop and waits for all tasks.
- Each stream returns `StreamImpl { standalone: true }`. Senders stay alive until `close`.
- Emitted lines are `(String, Duration since UNIX_EPOCH)`.
- stdin/stdout are reserved for remoc. Runtime logs go to stderr with `DEBUG `, `INFO `, `WARN ` or `ERROR ` prefixes.

## Configuration

```yaml
plugins:
  syslog:
    path: /usr/bin/reaction-plugin-syslog
    systemd: false
    options:
      listen: '0.0.0.0:5140'
      protocol: udp
      parse: auto
      framing: auto
      reconnect_delay: 5s
      max_datagram_bytes: 65535
      max_line_bytes: 65536
      timestamp: receive
      max_timestamp_skew: 5s
      max_tcp_connections: 128
      tcp_idle_timeout: 60s

streams:
  auth:
    type: syslog
    options:
      select:
        facilities: [auth, authpriv]
        severities: [err, crit, alert, emerg]
      prefix: '{host} {appname}[{procid}]: '
    filters: {}
```

Plugin options:

| Key | Default | Description |
| --- | --- | --- |
| `listen` | required | Address to bind as `host:port`. One listener is shared by all streams. |
| `protocol` | `udp` | `udp` or `tcp`. |
| `parse` | `auto` | `auto`, `rfc3164`, `rfc5424` or `raw`. |
| `framing` | `auto` | TCP framing: `auto`, `newline` or `octet`. |
| `reconnect_delay` | `5s` | Retry delay after a listener error; greater than zero and at most `1h`. |
| `max_datagram_bytes` | `65535` | UDP receive buffer limit, `1..=1048576`. Does not apply to TCP. |
| `max_line_bytes` | `65536` | Maximum emitted line length, `1..=1048576`. |
| `timestamp` | `receive` | `receive` or `message`. |
| `max_timestamp_skew` | `5s` | Accepted difference between message and receive time, at most `1h`. |
| `max_tcp_connections` | `128` | Concurrent TCP connections, `1..=4096`. |
| `tcp_idle_timeout` | `60s` | Close a TCP connection after this idle time; greater than zero and at most `1h`. |

Stream options:

| Key | Default | Description |
| --- | --- | --- |
| `select` | empty | Select streams by `facilities`, `severities`, `hosts`, `appnames` or `procids`. |
| `prefix` | `""` | Template rendered from message metadata. |

Unknown fields, unknown facility/severity names and empty host/appname/procid values are rejected. Stream names cannot contain `.`; this is a core constraint.

## Message parsing and selection

- `auto` tries RFC 5424, then RFC 3164, then raw text. An explicit parser falls back to raw text when the envelope is malformed.
- Parsed metadata includes facility, severity, hostname, app name, process ID, message ID, structured data and source address.
- RFC 5424 timestamps are parsed for `timestamp: message`. RFC 3164 has no year or timezone, so it falls back to receive time.
- Facility and severity names are normalized; aliases such as `warn` and `error` are accepted.
- Host, app name and process ID matching is case-insensitive.
- Non-empty selector fields are ANDed; values inside one field are ORed. An empty `select` matches all messages.
- One message is sent to every matching stream. Streams that do not match receive nothing.

Prefix variables: `{facility}`, `{severity}`, `{host}`, `{appname}`, `{procid}`, `{msgid}`, `{source_addr}` and `{sd:KEY}`. Use `{{` and `}}` for literal braces. Control characters are removed from rendered values and the prefix is capped at 4096 bytes.

## Line handling and time

- UDP: one datagram becomes one message. Trailing NUL and CR bytes are removed; invalid UTF-8 is dropped, valid `U+FFFD` is preserved, and ANSI escape sequences are stripped.
- TCP: `framing: newline` reads newline-terminated messages; `framing: octet` reads `<length> <message>` frames. `auto` accepts both, so a raw line beginning with `<digits> ` is ambiguous; use `newline` for raw syslog relays.
- Frames split across reads are reassembled. Oversized frames and lines are dropped.
- UDP datagrams larger than `max_datagram_bytes` are dropped; TCP lines are bounded by `max_line_bytes` only.
- Raw or rendered lines larger than `max_line_bytes` are dropped.
- `timestamp: receive` uses plugin receive time and keeps `retryperiod` and delayed-action scheduling monotonic. `timestamp: message` uses the RFC 5424 timestamp only when it is within `max_timestamp_skew` of receive time; otherwise receive time is used.
- A full stream channel drops new lines instead of blocking the listener.
- Repeated warnings are logged at most once per second per reason.

## Runtime behavior

- UDP receiver and TCP listener errors are logged to stderr and retried after `reconnect_delay`. Retry sleeps stop immediately on shutdown.
- Each TCP connection is handled in its own tracked task. Connections are capped by `max_tcp_connections` and closed after `tcp_idle_timeout` without input.
- The TCP listener is also tracked, so `close()` waits for it to release the port.
- Stream senders remain alive even when no message matches, because core does not restart plugin streams.

## Deployment

- The plugin path must be absolute. In containers, set `systemd: false` because `run0` is unavailable.
- Port 514 is privileged. Use a non-privileged port such as 5140/6514 or grant `CAP_NET_BIND_SERVICE`.
- The plugin does not read `/dev/log` or tail files. Forward local syslog through a socket or use a syslog relay.

## Testing

Tests use in-process UDP/TCP senders and do not require an external syslog service.

```shell
cargo test --locked -p reaction-plugin-syslog
cargo test --locked --workspace
cargo build --locked --release --workspace
docker build --check -f Dockerfile.release .
```

Coverage includes configuration validation, UTF-8 boundaries, overflow-safe framing, RFC 3164/RFC 5424 parsing, raw fallback, selectors, fan-out, prefix safety, TCP limits, timestamp skew, backpressure and shutdown.

## Module layout

```text
plugins/reaction-plugin-syslog/
├── README.md
├── Cargo.toml
└── src/
    ├── main.rs          # main_loop(Plugin::default())
    ├── plugin.rs        # PluginInfo implementation and stream sinks
    ├── config.rs        # plugin/stream options and selectors
    ├── message.rs       # syslog parsing and metadata
    ├── template.rs      # safe prefix rendering
    ├── lines.rs         # byte normalization and TCP framing
    ├── dispatch.rs      # metadata fan-out and timestamp handling
    ├── log.rs           # stderr logging and throttling
    ├── backend/
    │   ├── mod.rs       # cancellable retry sleep
    │   ├── udp.rs
    │   └── tcp.rs
    └── tests.rs         # in-process UDP/TCP tests
```

The binary is wired into `Cargo.toml`, `Dockerfile.release` and `release.py`. Integration configuration lives in `tests/test-conf/test-syslog.jsonnet`. Repository-wide contracts are in [`AGENTS.md`](../../AGENTS.md); plugin-specific behavior stays in this file.
