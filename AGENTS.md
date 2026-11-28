# AGENTS.md

This file is the development handoff for AI agents. It describes the current code, its contracts and the constraints that must be preserved.

> **Spec version:** 1.0 — contributors and AI agents must read this file before making changes.

Quick index: [Purpose](#purpose) · [Architecture](#architecture) · [Plugin API](#plugin-api) · [Build and deployment](#build-and-deployment) · [Development and contribution rules](#development-and-contribution-rules) · [Commands](#commands)

## Purpose

reaction is a daemon that watches command output for repeated patterns and runs actions, commonly IP bans. Configuration is YAML or JSONnet. Core concepts are streams, filters, patterns, actions and plugins.

Repository features:

- Multi-architecture container images (`linux/amd64`, `linux/arm64`) with an inert default config and a Compose deployment example.
- Config directory/file permission checks and CLI socket path validation with a request timeout.

## Architecture

### Core

- `src/main.rs`, `src/lib.rs`: daemon and library entrypoints.
- `src/cli.rs`: CLI commands (`start`, `show`, `flush`, `trigger`, `rotate`, `test-config`, `test-regex`, `test-filter`).
- `src/protocol.rs`: client/daemon message serialization.
- `src/concepts/`: configuration model and the stream/filter/pattern/action/plugin types.
- `src/client/`: client-side command implementations.
- `src/daemon/`: async runtime; `stream.rs` dispatches lines to filters, `filter/` holds match state and triggers actions, `socket.rs` serves clients, `plugin/mod.rs` manages plugin processes.
- `crates/treedb/`: append-only JSON-line persistence with `Tree`/`Database` APIs; `raw.rs` handles files and serialization, `time.rs` defines shared time types.

### Plugin API

- `plugins/reaction-plugin/`: shared plugin API and helpers (`shutdown.rs`, `time.rs`, `merge.rs`, `line.rs`).
- Lifecycle: `manifest` → `load_config` (validation only) → `start` → `close`. An error in `start` aborts reaction startup.
- `load_config` returns one `remoc::rch::mpsc::Receiver<Line>` per stream; `Line = (String, Duration since UNIX epoch)`.
- Plugin streams have no `eof`/restart handling in core. The plugin keeps senders alive until `close` and owns reconnection logic.
- A line's `Duration` is the filter's current time, used by `retryperiod` and scheduled actions. `timestamp: receive` is the default for this reason.
- Runtime logs go to stderr with `DEBUG `/`INFO `/`WARN `/`ERROR ` prefixes; stdin/stdout are reserved for remoc.

## Build and deployment

- `Dockerfile.release`: production image; includes all shipped binaries, `config/docker.yml`, `LICENSE` and runtime dependencies.
- `docker-compose.example.yml`: deployment example with host networking, `CAP_NET_ADMIN` and an optional restricted Docker socket proxy.
- `.github/workflows/docker-publish.yml`: multi-architecture image workflow triggered by `main` and `v*` tags.
- Plugin paths must be absolute. Plugins running inside containers need `systemd: false` because `run0` is unavailable.
- Default paths: config `/etc/reaction`, state `/var/lib/reaction`, socket `/run/reaction/reaction.sock`.
- Firewall plugins need host networking and `CAP_NET_ADMIN`; iptables must use the nft backend.

## Conventions

- Config files use `.yml`, `.yaml`, `.json` or `.jsonnet`. Inside a config directory, top-level files with those extensions are loaded.
- Keep builds reproducible with `--locked`.
- `n_nodes_simultaneous_startup` is ignored because of an upstream QUIC connection race.
- Inherited configuration and CLI documentation lives upstream; this file documents only repository-specific contracts.
- `README.md` is upstream-facing. Preserve upstream project information (Rationale, Help, Funding and upstream links) when editing it; add fork-specific notes without removing upstream content.

## Development and contribution rules

This section is the authoritative contributor/AI workflow for this fork. It applies to code, tests and documentation.

### Before starting work

- Read this file and `ARCHITECTURE.md`. For substantial features, open an issue first.
- This file takes precedence over `ARCHITECTURE.md` and upstream documentation when they disagree.

### Documentation rules

- Documentation and code comments describe current behavior, not development history or process.
- Keep documentation concise and high-signal: record only information needed to understand or change the project. Do not record AI conversation logs, temporary plans or unrelated details.
- Repository-specific user-facing behavior changes update `README.md` and `CHANGELOG.md`; inherited configuration/CLI reference remains upstream.
- Plugin-specific behavior and options live in `plugins/reaction-plugin-*/README.md`; consult them only when working on that plugin.
- Contract changes must update this file in the same change.

### Test requirements

- Every behavior change needs a test in the nearest relevant module.
- A new plugin needs unit tests with an in-process mock of its external dependency, plus at least one integration test configuration under `tests/test-conf/`.
- Tests must not require external services by default. If an external service is unavoidable, isolate it behind a feature flag or provide a mock.
- Test names describe the behavior being verified, not implementation details.

### Git workflow

- Branch from `main`; keep one logical change per commit.
- Branch names: `feat/`, `fix/`, `docs/`, `test/`, `refactor/` or `chore/` followed by a short description.
- Use Conventional Commits:

  ```text
  feat(plugin): add UDP listener
  fix: strip ANSI escape codes
  docs: clarify AI reading order
  test(plugin): cover framing
  refactor(core): simplify filter setup
  chore: bump dependencies
  ```

- Release/version commits use `vX.Y.Z`.
- Keep the subject line at most 72 characters, imperative mood, lowercase scope and no trailing period.
- The body explains why the change is needed, not a step-by-step narration. Reference issues with `Refs: #123`.
- Do not include AI transcripts, generated logs or unrelated files in commits.
- Rebase onto `main` before opening a pull request. Do not force-push shared branches.

### Pull requests

Describe:

1. the problem or motivation;
2. the behavior change;
3. tests added or updated;
4. documentation updated;
5. commands run locally.

Link the related issue and keep the pull request focused on one logical change.

## Commands

```shell
cargo build --locked --release --workspace
cargo test --locked --workspace
cargo run -- test-config -c <path>
docker build --check -f Dockerfile.release .
docker build -f Dockerfile.release -t reaction .
```

The full test suite requires `libipset-dev` and `libnftables-dev`.
