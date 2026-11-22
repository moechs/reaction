# AGENTS.md

This file is the development handoff for AI agents. It describes the current code, its contracts and the constraints that must be preserved.

## Purpose

reaction is a daemon that watches command output for repeated patterns and runs actions, commonly IP bans. Configuration is YAML or JSONnet. Core concepts are streams, filters, patterns, actions and plugins.

Repository features:

- `reaction-plugin-container`: Docker log streams selected by container name or label, with cursor-based resume and prefix templates.
- Multi-architecture container images (`linux/amd64`, `linux/arm64`) with an inert default config and a Compose deployment example.
- Config directory/file permission checks and CLI socket path validation with a request timeout.

## Architecture

### Core

- `src/main.rs`, `src/lib.rs`: daemon and library entrypoints.
- `src/cli.rs`: CLI commands (`start`, `show`, `flush`, `trigger`, `test-config`, `test-regex`, `test-filter`).
- `src/protocol.rs`: client/daemon message serialization.
- `src/concepts/`: configuration model and the stream/filter/pattern/action/plugin types.
- `src/client/`: client-side command implementations.
- `src/daemon/`: async runtime; `stream.rs` dispatches lines to filters, `filter/` holds match state and triggers actions, `socket.rs` serves clients, `plugin.rs` manages plugin processes.
- `crates/treedb/`: append-only JSON-line persistence with `Tree`/`Database` APIs; `raw.rs` handles files and serialization, `time.rs` defines shared time types.

### Plugin API

- `plugins/reaction-plugin/`: shared plugin API and helpers (`shutdown.rs`, `parse_duration.rs`).
- Lifecycle: `manifest` → `load_config` (validation only) → `start` → `close`. An error in `start` aborts reaction startup.
- `load_config` returns one `remoc::rch::mpsc::Receiver<Line>` per stream; `Line = (String, Duration since UNIX epoch)`.
- Plugin streams have no `eof`/restart handling in core. The plugin keeps senders alive until `close` and owns reconnection logic.
- A line's `Duration` is the filter's current time, used by `retryperiod` and scheduled actions. `timestamp: receive` is the default for this reason.
- Runtime logs go to stderr with `DEBUG `/`INFO `/`WARN `/`ERROR ` prefixes; stdin/stdout are reserved for remoc.

### Container stream plugin

- `plugins/reaction-plugin-container/src/plugin.rs`: `PluginInfo`, plugin options, backend startup.
- `src/core/runtime.rs`: backend contract (`Runtime`, `RuntimeEvent`, `LogLine`); no Docker types in `core/`.
- `src/core/config.rs`: per-stream options (`match`, `exclude`, `sources`, `since`, `timestamp`, `prefix`, `max_line_bytes`).
- `src/core/selector.rs`: name/label match and exclude semantics.
- `src/core/template.rs`: prefix templates (`{name}`, `{id}`, `{short_id}`, `{image}`, `{source}`, `{label:KEY}`).
- `src/core/lines.rs`: chunk-to-line reassembly, truncation and UTF-8 handling.
- `src/core/cursor.rs`: resume position and replay de-duplication.
- `src/core/supervisor.rs`: discovery, reconciliation and one follower task per stream/container.
- `src/backend/docker.rs`: bollard-based Engine API backend (snapshot, events, logs, reconnects, self-detection via `/proc/self/mountinfo`).
- `src/tests.rs`: plugin tests with an in-process mock Docker API; no Docker daemon needed.

## Build and deployment

- `Dockerfile.release`: production image; includes all shipped binaries, `config/docker.yml`, `LICENSE` and runtime dependencies.
- `docker-compose.example.yml`: deployment example with host networking, `CAP_NET_ADMIN` and an optional restricted Docker socket proxy.
- `.github/workflows/docker-publish.yml`: multi-architecture image workflow triggered by `main` and `v*` tags.
- Plugin paths must be absolute. Plugins running inside containers need `systemd: false` because `run0` is unavailable.
- Default paths: config `/etc/reaction`, state `/var/lib/reaction`, socket `/run/reaction/reaction.sock`.
- Firewall plugins need host networking and `CAP_NET_ADMIN`; iptables must use the nft backend.

## Conventions

- Config files use `.yml` or `.jsonnet`. Inside a config directory, top-level files with those extensions are loaded.
- Keep builds reproducible with `--locked`.
- `n_nodes_simultaneous_startup` is ignored because of an upstream QUIC connection race.
- Inherited configuration and CLI documentation lives upstream; this file documents only repository-specific contracts.
- Code comments and documentation describe current behavior, not development history or process.

## Commands

```shell
cargo build --locked --release --workspace
cargo test --locked --workspace
cargo test --locked -p reaction-plugin-container
cargo run -- test-config -c <path>
docker build --check -f Dockerfile.release .
docker build -f Dockerfile.release -t reaction .
```

The full test suite requires `libipset-dev` and `libnftables-dev`. The container plugin tests need neither those libraries nor a Docker daemon.
