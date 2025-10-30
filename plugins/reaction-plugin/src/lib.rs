//! This crate defines the API between reaction's core and plugins.
//!
//! To implement a plugin, one has to provide an implementation of [`PluginInfo`], that provides
//! the entrypoint for a plugin.
//! It permits to define 0 to n (stream, filter, action) custom types.
//!
//! Minimal example:
//! `src/main.rs`
//! ```rust
//! use reaction_plugin::PluginInfo;
//!
//! #[tokio::main]
//! async fn main() {
//!     let plugin = MyPlugin::default();
//!     reaction_plugin::main_loop(plugin).await;
//! }
//!
//! #[derive(Default)]
//! struct MyPlugin {}
//!
//! impl PluginInfo for Plugin {
//!   // ...
//! }
//! ```
//!
//! ## Naming & calling conventions
//!
//! Your plugin should be named `reaction-plugin-$NAME`, eg. `reaction-plugin-postgresql`.
//! It will be invoked with one positional argument "serve".
//! ```
//! reaction-plugin-$NAME serve
//! ```
//! This can be useful if you want to provide CLI functionnality to your users.
//!
//! It will be run in its own directory, in which it should have write access.
//!
//! ## Communication
//!
//! Communication between the plugin and reaction is based on [`remoc`], which permits to multiplex channels and remote objects/functions/trait
//! calls over a single transport channel.
//! The channels used are stdin and stdout, so you can't use them for something else.
//!
//! ### Errors
//!
//! Errors can be printed to stderr.
//! They'll be captured line by line and re-printed by reaction, with the plugin name prepended.
//!
//! A line can start with `DEBUG `, `INFO `, `WARN `, `ERROR `.
//! If the starts with none of the above, the line is assumed to be an error.
//!
//! Examples:
//! ```log
//! WARN This is an official warning from the plugin
//! # will become:
//! WARN plugin test: This is an official warning from the plugin
//!
//! Freeeee errrooooorrr
//! # will become:
//! ERROR plugin test: Freeeee errrooooorrr
//! ```
//!
//! ## Starting template
//!
//! Core plugins can be found here: <https://framagit.org/ppom/reaction/-/tree/main/plugins>
//! The "virtual" plugin is the simplest and can serve as a template.
//! You'll have to adjust dependencies versions in `Cargo.toml`.

use std::{collections::BTreeSet, error::Error, fmt::Display, process::exit};

use remoc::{
    Connect, rch,
    rtc::{self, Server},
};
use serde::{Deserialize, Serialize};
pub use serde_json::Value;
use tokio::io::{stdin, stdout};

/// This is the only trait that **must** be implemented by a plugin.
/// It provides lists of stream, filter and action types implemented by a dynamic plugin.
#[rtc::remote]
pub trait PluginInfo {
    /// Return the manifest of the plugin.
    async fn manifest(&mut self) -> Result<Manifest, rtc::CallError>;

    /// Return one stream of a given type if it exists
    async fn stream_impl(
        &mut self,
        stream_name: String,
        stream_type: String,
        config: Value,
    ) -> RemoteResult<StreamImpl>;

    /// Return one instance of a given type.
    async fn action_impl(
        &mut self,
        stream_name: String,
        filter_name: String,
        action_name: String,
        action_type: String,
        config: Value,
        patterns: Vec<String>,
    ) -> RemoteResult<ActionImpl>;

    /// Notify the plugin that setup is finished, permitting a last occasion to report an error
    /// (For example if a stream wants a companion action but it hasn't been initialized)
    /// All initialization (opening remote connections, starting streams, etc) should happen here.
    async fn finish_setup(&mut self) -> RemoteResult<()>;

    /// Notify the plugin that reaction is quitting and that the plugin should quit too.
    /// A few seconds later, the plugin will receive SIGTERM.
    /// A few seconds later, the plugin will receive SIGKILL.
    async fn close(mut self) -> RemoteResult<()>;
}

#[derive(Serialize, Deserialize)]
pub struct Manifest {
    // Protocol version. available as the [`hello!`] macro.
    pub hello: Hello,
    /// stream types that should be made available to reaction users
    /// ```jsonnet
    /// {
    ///   streams: {
    ///     my_stream: {
    ///       type: "..."
    ///       # ↑ all those exposed types
    ///     }
    ///   }
    /// }
    /// ```
    pub streams: BTreeSet<String>,
    /// All action types that should be made available to reaction users
    pub actions: BTreeSet<String>,
}

#[derive(Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hello {
    /// Major version of the protocol
    /// Increment means breaking change
    pub version_major: u32,
    /// Minor version of the protocol
    /// Increment means reaction core can handle older version plugins
    pub version_minor: u32,
}

impl Hello {
    pub fn new() -> Hello {
        let mut version = env!("CARGO_PKG_VERSION").split(".");
        Hello {
            version_major: version.next().unwrap().parse().unwrap(),
            version_minor: version.next().unwrap().parse().unwrap(),
        }
    }

    pub fn is_compatible(server: &Hello, plugin: &Hello) -> std::result::Result<(), String> {
        if server.version_major == plugin.version_major
            && server.version_minor >= plugin.version_minor
        {
            Ok(())
        } else if plugin.version_major > server.version_major
            || (plugin.version_major == server.version_major
                && plugin.version_minor > server.version_minor)
        {
            Err("consider upgrading reaction".into())
        } else {
            Err("consider upgrading the plugin".into())
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct StreamImpl {
    pub stream: rch::mpsc::Receiver<String>,
    /// Whether this stream works standalone, or if it needs other streams to be fed.
    /// Defaults to true.
    /// When false, reaction will exit if it's the last one standing.
    #[serde(default = "_true")]
    pub standalone: bool,
}

fn _true() -> bool {
    true
}

// #[derive(Serialize, Deserialize)]
// pub struct FilterImpl {
//     pub stream: rch::lr::Sender<Exec>,
// }

// #[derive(Serialize, Deserialize)]
// pub struct Match {
//     pub match_: String,
//     pub result: rch::oneshot::Sender<bool>,
// }

#[derive(Clone, Serialize, Deserialize)]
pub struct ActionImpl {
    pub tx: rch::mpsc::Sender<Exec>,
}

#[derive(Serialize, Deserialize)]
pub struct Exec {
    pub match_: Vec<String>,
    pub result: rch::oneshot::Sender<Result<(), String>>,
}

// TODO write main function here?
pub async fn main_loop<T: PluginInfo + Send + Sync + 'static>(plugin_info: T) {
    let (conn, mut tx, _rx): (
        _,
        remoc::rch::base::Sender<PluginInfoClient>,
        remoc::rch::base::Receiver<()>,
    ) = Connect::io(remoc::Cfg::default(), stdin(), stdout())
        .await
        .unwrap();

    let (server, client) = PluginInfoServer::new(plugin_info, 1);

    let (res1, (_, res2), res3) = tokio::join!(tx.send(client), server.serve(), tokio::spawn(conn));
    let mut exit_code = 0;
    if let Err(err) = res1 {
        eprintln!("ERROR could not send plugin info to reaction: {err}");
        exit_code = 1;
    }
    if let Err(err) = res2 {
        eprintln!("ERROR could not launch plugin service for reaction: {err}");
        exit_code = 2;
    }
    if let Err(err) = res3 {
        eprintln!("ERROR could not setup connection with reaction: {err}");
        exit_code = 3;
    } else if let Ok(Err(err)) = res3 {
        eprintln!("ERROR could not setup connection with reaction: {err}");
        exit_code = 3;
    }
    exit(exit_code);
}

// Errors

pub type RemoteResult<T> = Result<T, RemoteError>;

/// A Plugin Error
/// It's either a connection error or a free String for plugin-specific errors
#[derive(Debug, Serialize, Deserialize)]
pub enum RemoteError {
    Remoc(rtc::CallError),
    Plugin(String),
}

impl Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::Remoc(call_error) => write!(f, "communication error: {call_error}"),
            RemoteError::Plugin(err) => write!(f, "{err}"),
        }
    }
}

impl Error for RemoteError {}

impl From<String> for RemoteError {
    fn from(value: String) -> Self {
        Self::Plugin(value)
    }
}

impl From<&str> for RemoteError {
    fn from(value: &str) -> Self {
        Self::Plugin(value.into())
    }
}

impl From<rtc::CallError> for RemoteError {
    fn from(value: rtc::CallError) -> Self {
        Self::Remoc(value)
    }
}
