# reaction-plugin-container

Docker Engine API input plugin for reaction. It follows running containers, reads stdout and stderr through the Engine API, and emits reaction lines to streams. It is a pure input plugin; filtering and actions stay in reaction core.

## Contract with reaction core

- `manifest()` advertises stream type `docker` and no action types.
- `load_config()` validates options and creates one `remoc::rch::mpsc::channel` per stream. It does not connect to Docker.
- `start()` connects to Docker unless `wait_for_runtime` is enabled. A connection or ping error aborts reaction startup by default.
- `close()` asks the shutdown controller to stop and waits for all tasks.
- Each stream returns `StreamImpl { standalone: true }`. Senders stay alive until `close`, including when no container matches.
- Emitted lines are `(String, Duration since UNIX_EPOCH)`.
- stdin/stdout are reserved for remoc. Runtime logs go to stderr with `DEBUG `, `INFO `, `WARN ` or `ERROR ` prefixes.

## Configuration

```yaml
plugins:
  container:
    path: /usr/bin/reaction-plugin-container
    systemd: false
    options:
      reconnect_delay: 5s
      wait_for_runtime: false
      docker:
        host: unix:///var/run/docker.sock

streams:
  containers:
    type: docker
    options:
      match:
        names: [web, api]
        labels: [app=shop, tier]
      exclude:
        labels: [reaction.ignore]
      sources: [stdout, stderr]
      since: now
      timestamp: receive
      prefix: '{name}[{short_id}] {source}: '
```

Plugin options:

| Key | Default | Description |
| --- | --- | --- |
| `reconnect_delay` | `5s` | Delay before retrying a failed runtime connection, event stream or log stream; must be greater than zero. |
| `wait_for_runtime` | `false` | When `false`, an unreachable Docker daemon aborts startup. When `true`, the plugin retries in the background. |
| `docker.host` | `$DOCKER_HOST`, then `unix:///var/run/docker.sock` | Optional Docker Engine API address. Only `unix://`, `tcp://` and `http://` are accepted. |

Stream options:

| Key | Default | Description |
| --- | --- | --- |
| `match` | required | Container selector; must contain at least one name or label. |
| `exclude` | empty | Optional selector; matching containers are never followed. |
| `sources` | `[stdout, stderr]` | Non-empty subset of `stdout` and `stderr`. |
| `since` | `now` | `now` or a duration such as `10m` to include older lines. |
| `timestamp` | `receive` | `receive` or `runtime`. |
| `prefix` | `""` | Template rendered from container metadata. |
| `max_line_bytes` | `65536` | Maximum log-content length before the remainder is discarded, `1..=1048576`. |

Unknown fields are rejected. Stream names cannot contain `.`; this is a core constraint.

## Container selection

- Names match exactly; any listed name can match.
- Labels use `key=value` for an exact value or bare `key` to require the key to exist. All listed labels must match.
- The `names` and `labels` dimensions are ORed: matching either dimension is enough.
- `exclude` uses the same rules and wins over `match`. An empty selector matches nothing.

## Prefixes and line handling

Prefix variables: `{id}`, `{short_id}` (first 12 characters), `{name}`, `{image}`, `{source}`, and `{label:KEY}`. Missing labels render as an empty string. Use `{{` and `}}` for literal braces. Unknown variables and malformed templates are rejected at config load time. Metadata values are sanitized to remove control characters and ANSI escape sequences; the rendered prefix is capped at 4096 bytes.

- Logs are read with `follow=1&timestamps=1`. Docker's per-chunk timestamps are stripped from the line text.
- Both multiplexed non-TTY output and raw TTY output are decoded.
- Lines keep the timestamp of the chunk in which they started. Trailing `\r`, invalid UTF-8 and ANSI escape sequences are removed from the line text.
- Lines longer than `max_line_bytes` are truncated; the rest of the line is discarded until the next newline.
- `timestamp: receive` uses plugin receive time and stays monotonic when several containers feed one stream. `timestamp: runtime` uses Docker's timestamp when available, falling back to receive time.
- Stream sends are awaited with shutdown and cancellation awareness. A send error stops that container's follower and is logged to stderr.

## Runtime behavior

- Discovery starts with a container snapshot, then follows Docker events for `start`, `die`, `destroy` and `rename`. Snapshot reconciliation removes followers for containers that disappeared while events were unavailable.
- Event and log-stream failures are logged and retried after `reconnect_delay`. Retry sleeps stop immediately on shutdown.
- The plugin detects its own container through `/proc/self/mountinfo` and never follows itself.
- Cursors are in-memory only. A reopened log stream drops replayed lines older than the cursor and deduplicates lines at the cursor timestamp with a 64-entry hash window. Once a newer line arrives, replay ends and live lines pass even if Docker timestamps later go backwards. Cursor state is not restored after a plugin restart.
- `since` is measured from the runtime connection time, not from container discovery time. With `wait_for_runtime: true`, that is the time Docker became reachable.

## Deployment

- The plugin path must be absolute. In containers, set `systemd: false` because `run0` is unavailable.
- The plugin reads logs through the Engine API only; it does not need direct access to `/var/lib/docker/containers`.
- Docker socket access is root-equivalent and `:ro` does not restrict the API. Prefer a read-only socket proxy allowing only `/containers/json`, `/events`, `/containers/{id}/logs` and `/containers/{id}/json`. See [`docker-compose.example.yml`](../../docker-compose.example.yml).

## Testing

Plugin tests use an in-process mock Docker Engine API on a temporary Unix socket and do not require a Docker daemon.

```shell
cargo test --locked -p reaction-plugin-container
cargo test --locked --workspace
cargo build --locked --release --workspace
docker build --check -f Dockerfile.release .
```

Coverage includes configuration validation, selectors, prefixes, line assembly, cursors, discovery, reconnects, timestamp handling, `wait_for_runtime` and shutdown.

## Module layout

```text
plugins/reaction-plugin-container/
├── Cargo.toml
└── src/
    ├── main.rs          # main_loop(Plugin::default())
    ├── plugin.rs        # PluginInfo implementation and stream sinks
    ├── log.rs           # stderr logging macros
    ├── tests.rs         # plugin tests and mock Docker Engine API
    ├── backend/
    │   ├── mod.rs
    │   └── docker.rs    # Docker Engine API client and self-container detection
    └── core/
        ├── mod.rs
        ├── config.rs    # plugin/stream options
        ├── cursor.rs    # replay and deduplication
        ├── lines.rs     # line assembly and normalization
        ├── meta.rs      # container metadata
        ├── runtime.rs   # backend traits
        ├── selector.rs  # container selection
        ├── supervisor.rs # followers, retries and shutdown
        └── template.rs  # safe prefix rendering
```

The binary is wired into `Cargo.toml`, `Dockerfile.release` and `release.py`. Repository-wide contracts are in [`AGENTS.md`](../../AGENTS.md); plugin-specific behavior stays in this file.
