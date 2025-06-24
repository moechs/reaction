# Architecture

Here is a high-level overview of the codebase.

*Don't hesitate to create an issue or a merge request if something is unclear, missing or outdated.*

## Build

- `build.rs`: permits to create shell completions and man pages on build.
- `Cargo.toml`, `Cargo.lock`: manifest and dependencies.
- `config/`: example / test configuration files. Look at its git history to discover more.
- `Makefile`: Makefile. Resumes useful commands.
- `packaging/`: Files useful for .deb and .tar generation.
- `release.py`: Build process for a release. Handles cross-compilation, .tar and .deb generation.

## Main source code

- `helpers_c/`: C helpers. I wish to have special IP support in reaction and get rid of them. See #79 and #116.
- `tests/`: Integration tests. For now they test basic reaction runtime behavior, persistance, and client-daemon communication.
- `src/`: The source code, here we go!

### Top-level files

- `src/main.rs`: Main entrypoint
- `src/lib.rs`: Second main entrypoint
- `src/cli.rs`: Command-line arguments
- `src/tests.rs`: Test utilities

### `src/concepts/`

reaction really is about its configuration, which is at the center of the code.

There is one file for each of its concepts: configuration, streams, filters, actions, patterns.

### `src/protocol/`

Low-level serialization/deserialization and client-daemon protocol messages.

Shared by the client and daemon's socket. Also used by daemon's database.

### `src/client/`

Client code: `reaction show`, `reaction flush`, `reaction trigger`, `reaction test-regex`.

- `request.rs`: commands requiring client/server communication: `show`, `flush` & `trigger`.
- `test_config.rs`: `test-config` command.
- `test_regex.rs`: `test-regex` command.

### `src/daemon`

Daemon runtime structures and logic.

This code has async code, to handle input streams and communication with clients, using the tokio runtime.

- `mod.rs`: daemon main function. Initializes all tasks, handles synchronization and quitting, etc.
- `stream.rs`: Stream managers: start the stream `cmd` and dispatch its stdout lines to its Filter managers.
- `filter/`: Filter managers: handle lines, persistance, store matches and trigger actions. This is the main piece of runtime logic.
    - `mod.rs`: High-level logic
    - `state.rs`: Inner state operations
- `socket.rs`: The socket task, responsible for communication with clients.
- `shutdown.rs`: Logic for passing shutdown signal across all tasks

### `src/tree`

Persistence layer.

This is a database highly adapted to reaction workload, making reaction faster than when used with general purpose key-value databases
(heed, sled and fjall crates have been tested).
Its design is explained in the comments of its files:

- `mod.rs`: main database code, with its two API structs: Tree and Database.
- `raw.rs` low-level part, directly interacting with de/serializisation and files.
