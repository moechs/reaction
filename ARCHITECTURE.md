# Architecture

Here is a high-level overview of the codebase.

*Don't hesitate to create an issue or a merge request if something is unclear, missing or outdated.*

## Build

- `build.rs`: permits to create shell completions and man pages on build.
- `Cargo.toml`, `Cargo.lock`: manifest and dependencies.
- `config`: example / test configuration files. Look at the git history to discover more!
- `debian`: reaction.deb generation.
- `Makefile`: Makefile. I plan to remove this at some point.
- `release.py`: Build process for a release. I'd like to make it more modular, to permit to build specific parts only for example.

## Main source code

- `helpers_c`: C helpers. I wish to have special IP support in reaction and get rid of them.
- `tests`: Integration tests. For now they test basic reaction runtime behavior, persistance, and client-daemon communication.
- `src`: The source code, here we go!

### Top-level files

- `src/main.rs`: Main entrypoint
- `src/lib.rs`: Second main entrypoint
- `src/cli.rs`: Command-line arguments
- `src/tests.rs`: Test utilities

### `src/concepts`

reaction really is about its configuration, which is at the center of the code.

There is one file for each of its concepts: configuration, streams, filters, actions, patterns.

### `src/protocol`

Low-level serialization/deserialization and client-daemon protocol messages.

Shared by the client and daemon's socket. Also used by daemon's database.

### `src/client`

Client code: `reaction show`, `reaction flush`, `reaction test-regex`.

- `show_flush.rs`: `show` & `flush` commands.
- `test_regex.rs`: `test-regex` command.

### `src/daemon`

Daemon runtime structures and logic.

This code is mainly async, with the tokio runtime.

- `mod.rs`: daemon main function. Initializes all tasks, handles synchronization and quitting, etc.
- `stream.rs`: Stream managers: start the stream `cmd` and dispatch its stdout lines to its Filter managers.
- `filter.rs`: Filter managers: handle lines, store matches, send logs to database and decide when to trigger actions.
- `action.rs`: Action managers: handle action triggers (*execs*), store & manage pending actions.
- `socket.rs`: The socket task, responsible for communication with clients.
- `database`: The database thread. This is a sync thread, because it's somehow muuch faster. At startup it sends persisted matches to the Filter managers. Then it receives match/exec logs from the filters and persist them.
    - `database/mod.rs`: Main logic.
    - `database/lowlevel.rs`: Low-level implementation details (serialization / deserialization and size optimizations).
    - `database/tests.rs`: Unit tests.

## Migration from Go to Rust

- `go.old/`: Go / v1 codebase.

Those scripts are merged in a single-file executable by `release.py`:

- `export-go-db/`: Go script to export the reaction-v1 database as JSON.
- `import-rust-db/`: Rust script to import the JSON export as a reaction-v2 database.
