//! This crate defines the API between reaction's core and plugins.
//!
//! Plugins must be written in Rust, for now.
//!
//! This documentation assumes the reader has some knowledge of Rust.
//! However, if you find that something is unclear, don't hesitate to
//! [ask for help](https://framagit.org/ppom/reaction/#help), even if you're new to Rust.
//!
//! To implement a plugin, one has to provide an implementation of [`PluginInfo`], that provides
//! the entrypoint for a plugin.
//! It permits to define `0` to `n` custom stream and action types.
//!
//! ## Note on reaction-plugin API stability
//!
//! This is the v1 of reaction's plugin interface.
//! It's quite efficient and complete, but it has the big drawback of being Rust-only and [`tokio`]-only.
//!
//! In the future, I'd like to define a language-agnostic interface, which will be a major breaking change in the API.
//! However, I'll try my best to reduce the necessary code changes for plugins that use this v1.
//!
//! ## Naming & calling conventions
//!
//! Your plugin should be named `reaction-plugin-$NAME`, eg. `reaction-plugin-postgresql`.
//! It will be invoked with one positional argument "serve".
//! ```bash
//! reaction-plugin-$NAME serve
//! ```
//! This can be useful if you want to provide CLI functionnality to your users,
//! so you can distinguish between a human user and reaction.
//!
//! ### State directory
//!
//! It will be executed in its own directory, in which it should have write access.
//! The directory is `$reaction_state_directory/plugin_data/$NAME`.
//! reaction's [state_directory](https://reaction.ppom.me/reference.html#state_directory)
//! defaults to its working directory, which is `/var/lib/reaction` in most setups.
//!
//! So your plugin directory should most often be `/var/lib/reaction/plugin_data/$NAME`,
//! but the plugin shouldn't expect that and use the current working directory instead.
//!
//! ## Communication
//!
//! Communication between the plugin and reaction is based on [`remoc`], which permits to multiplex channels and remote objects/functions/trait
//! calls over a single transport channel.
//! The channels read and write channels are stdin and stdout, so you shouldn't use them for something else.
//!
//! [`remoc`] builds upon [`tokio`], so you'll need to use tokio too.
//!
//! ### Errors
//!
//! Errors during:
//! - config loading in [`PluginInfo::load_config`]
//! - startup in [`PluginInfo::start`]
//!
//! should be returned to reaction by the function's return value, permitting reaction to abort startup.
//!
//! During normal runtime, after the plugin has loaded its config and started, and before reaction is quitting, there is no *rusty* way to send errors to reaction.
//! Then errors can be printed to stderr.
//! They'll be captured line by line and re-printed by reaction, with the plugin name prepended.
//!
//! A line can start with `DEBUG `, `INFO `, `WARN `, `ERROR `.
//! If it starts with none of the above, the line is assumed to be an error.
//!
//! Example:
//! Those lines:
//! ```log
//! WARN This is an official warning from the plugin
//! Freeeee errrooooorrr
//! ```
//! Will become:
//! ```log
//! WARN plugin test: This is an official warning from the plugin
//! ERROR plugin test: Freeeee errrooooorrr
//! ```
//!
//! Plugins should not exit when there is an error: reaction quits only when told to do so,
//! or if all its streams exit, and won't retry starting a failing plugin or stream.
//! Please only exit if you're in a 100% failing state.
//! It's considered better to continue operating in a degraded state than exiting.
//!
//! ## Getting started
//!
//! If you don't have Rust already installed, follow their [*Getting Started* documentation](https://rust-lang.org/learn/get-started/)
//! to get rust build tools and learn about editor support.
//!
//! Then create a new repository with cargo:
//!
//! ```bash
//! cargo new reaction-plugin-$NAME
//! cd reaction-plugin-$NAME
//! ```
//!
//! Add required dependencies:
//!
//! ```bash
//! cargo add reaction-plugin tokio
//! ```
//!
//! Replace `src/main.rs` with those contents:
//!
//! ```ignore
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
//! impl PluginInfo for MyPlugin {
//!   // ...
//! }
//! ```
//!
//! Your IDE should now propose to implement missing members of the [`PluginInfo`] trait.
//! Your journey starts!
//!
//! ## Examples
//!
//! Core plugins can be found here: <https://framagit.org/ppom/reaction/-/tree/main/plugins>.
//!
//! - The "virtual" plugin is the simplest and can serve as a good complete example that links custom stream types and custom action types.
//! - The "ipset" plugin is a good example of an action-only plugin.

use std::{
    collections::{BTreeMap, BTreeSet},
    env::args,
    error::Error,
    fmt::Display,
    process::exit,
    time::Duration,
};

use remoc::{
    Connect, rch,
    rtc::{self, Server},
};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value as JValue};
use tokio::io::{stdin, stdout};

pub mod line;
pub mod shutdown;
pub mod time;

/// The only trait that **must** be implemented by a plugin.
/// It provides lists of stream, filter and action types implemented by a dynamic plugin.
#[rtc::remote]
pub trait PluginInfo {
    /// Return the manifest of the plugin.
    /// This should not be dynamic, and return always the same manifest.
    ///
    /// Example implementation:
    /// ```
    /// Ok(Manifest {
    ///     hello: Hello::new(),
    ///     streams: BTreeSet::from(["mystreamtype".into()]),
    ///     actions: BTreeSet::from(["myactiontype".into()]),
    /// })
    /// ```
    ///
    /// First function called.
    async fn manifest(&mut self) -> Result<Manifest, rtc::CallError>;

    /// Load all plugin stream and action configurations.
    /// Must error if config is invalid.
    ///
    /// The plugin should not start running mutable commands here:
    /// It should be ok to quit without cleanup for now.
    ///
    /// Each [`StreamConfig`] from the `streams` arg should result in a corresponding [`StreamImpl`] returned, in the same order.
    /// Each [`ActionConfig`] from the `actions` arg should result in a corresponding [`ActionImpl`] returned, in the same order.
    ///
    /// Function called after [`PluginInfo::manifest`].
    async fn load_config(
        &mut self,
        streams: Vec<StreamConfig>,
        actions: Vec<ActionConfig>,
    ) -> RemoteResult<(Vec<StreamImpl>, Vec<ActionImpl>)>;

    /// Notify the plugin that setup is finished, permitting a last occasion to report an error that'll make reaction exit.
    /// All initialization (opening remote connections, starting streams, etc) should happen here.
    ///
    /// Function called after [`PluginInfo::load_config`].
    async fn start(&mut self) -> RemoteResult<()>;

    /// Notify the plugin that reaction is quitting and that the plugin should quit too.
    /// A few seconds later, the plugin will receive SIGTERM.
    /// A few seconds later, the plugin will receive SIGKILL.
    ///
    /// Function called after [`PluginInfo::start`], when reaction is quitting.
    async fn close(mut self) -> RemoteResult<()>;
}

/// The config for one Stream of a type advertised by this plugin.
///
/// For example this user config:
/// ```jsonnet
/// {
///   streams: {
///     mystream: {
///       type: "mystreamtype",
///       options: {
///         key: "value",
///         num: 3,
///       },
///       // filters: ...
///     },
///   },
/// }
/// ```
///
/// would result in the following `StreamConfig`:
///
/// ```
/// StreamConfig {
///   stream_name: "mystream",
///   stream_type: "mystreamtype",
///   config: Value::Object(BTreeMap::from([
///     ("key", Value::String("value")),
///     ("num", Value::Integer(3)),
///   ])),
/// }
/// ```
///
/// Don't hesitate to take advantage of [`serde_json::from_value`], to deserialize the [`Value`] into a Rust struct:
///
/// ```
/// #[derive(Deserialize)]
/// struct MyStreamOptions {
///   key: String,
///   num: i64,
/// }
///
/// fn validate_config(stream_config: Value) -> Result<MyStreamOptions, serde_json::Error> {
///   serde_json::from_value(stream_config.into())
/// }
/// ```
#[derive(Serialize, Deserialize, Clone)]
pub struct StreamConfig {
    pub stream_name: String,
    pub stream_type: String,
    pub config: Value,
}

/// The config for one Stream of a type advertised by this plugin.
///
/// For example this user config:
/// ```jsonnet
/// {
///   streams: {
///     mystream: {
///       // ...
///       filters: {
///         myfilter: {
///           // ...
///           actions: {
///             myaction: {
///               type: "myactiontype",
///               options: {
///                 boolean: true,
///                 array: ["item"],
///               },
///             },
///           },
///         },
///       },
///     },
///   },
/// }
/// ```
///
/// would result in the following `ActionConfig`:
///
/// ```rust
/// ActionConfig {
///   action_name: "myaction",
///   action_type: "myactiontype",
///   config: Value::Object(BTreeMap::from([
///     ("boolean", Value::Boolean(true)),
///     ("array", Value::Array([Value::String("item")])),
///   ])),
/// }
/// ```
///
/// Don't hesitate to take advantage of [`serde_json::from_value`], to deserialize the [`Value`] into a Rust struct:
///
/// ```rust
/// #[derive(Deserialize)]
/// struct MyActionOptions {
///   boolean: bool,
///   array: Vec<String>,
/// }
///
/// fn validate_config(action_config: Value) -> Result<MyActionOptions, serde_json::Error> {
///   serde_json::from_value(action_config.into())
/// }
/// ```
#[derive(Serialize, Deserialize, Clone)]
pub struct ActionConfig {
    pub stream_name: String,
    pub filter_name: String,
    pub action_name: String,
    pub action_type: String,
    pub config: Value,
    pub patterns: Vec<String>,
}

/// Mandatory announcement of a plugin's protocol version, stream and action types.
#[derive(Serialize, Deserialize)]
pub struct Manifest {
    // Protocol version.
    // Just use the [`Hello::new`] constructor that uses this crate's current version.
    pub hello: Hello,
    /// Stream types that should be made available to reaction users
    ///
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
    /// Action types that should be made available to reaction users
    ///
    /// ```jsonnet
    /// {
    ///   streams: {
    ///     mystream: {
    ///       filters: {
    ///         myfilter: {
    ///           actions: {
    ///             myaction: {
    ///               type: "myactiontype",
    ///                # ↑ all those exposed types
    ///             },
    ///           },
    ///         },
    ///       },
    ///     },
    ///   },
    /// }
    /// ```
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
    /// Constructor that fills a [`Hello`] struct with [`crate`]'s version.
    /// You should use this in your plugin [`Manifest`].
    pub fn new() -> Hello {
        Hello {
            version_major: env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap(),
            version_minor: env!("CARGO_PKG_VERSION_MINOR").parse().unwrap(),
        }
    }

    /// Used by the reaction daemon. Permits to check compatibility between two versions.
    /// Major versions must be the same between the daemon and plugin.
    /// Minor version of the daemon must be greater than or equal minor version of the plugin.
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

/// A clone of [`serde_json::Value`].
/// Implements From & Into [`serde_json::Value`].
#[derive(Serialize, Deserialize, Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl From<JValue> for Value {
    fn from(value: serde_json::Value) -> Self {
        match value {
            JValue::Null => Value::Null,
            JValue::Bool(b) => Value::Bool(b),
            JValue::Number(number) => {
                if let Some(number) = number.as_i64() {
                    Value::Integer(number)
                } else if let Some(number) = number.as_f64() {
                    Value::Float(number)
                } else {
                    Value::Null
                }
            }
            JValue::String(s) => Value::String(s.into()),
            JValue::Array(v) => Value::Array(v.into_iter().map(|e| e.into()).collect()),
            JValue::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, v.into())).collect()),
        }
    }
}

impl Into<JValue> for Value {
    fn into(self) -> JValue {
        match self {
            Value::Null => JValue::Null,
            Value::Bool(v) => JValue::Bool(v),
            Value::Integer(v) => JValue::Number(v.into()),
            Value::Float(v) => JValue::Number(Number::from_f64(v).unwrap()),
            Value::String(v) => JValue::String(v),
            Value::Array(v) => JValue::Array(v.into_iter().map(|e| e.into()).collect()),
            Value::Object(m) => JValue::Object(m.into_iter().map(|(k, v)| (k, v.into())).collect()),
        }
    }
}

/// Represents a Stream handled by a plugin on reaction core's side.
///
/// During [`PluginInfo::load_config`], the plugin should create a [`remoc::rch::mpsc::channel`] of [`Line`].
/// It will keep the sending side for itself and put the receiving side in a [`StreamImpl`].
///
/// The plugin should start sending [`Line`]s in the channel only after [`PluginInfo::start`] has been called by reaction core.
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamImpl {
    pub stream: rch::mpsc::Receiver<Line>,
    /// Whether this stream works standalone, or if it needs other streams or actions to be fed.
    /// Defaults to true.
    /// When `false`, reaction will exit if it's the last one standing.
    #[serde(default = "_true")]
    pub standalone: bool,
}

fn _true() -> bool {
    true
}

/// Messages passed from the [`StreamImpl`] of a plugin to reaction core
pub type Line = (String, Duration);

// // Filters
// // For now, plugins can't handle custom filter implementations.
// #[derive(Serialize, Deserialize)]
// pub struct FilterImpl {
//     pub stream: rch::lr::Sender<Exec>,
// }
// #[derive(Serialize, Deserialize)]
// pub struct Match {
//     pub match_: String,
//     pub result: rch::oneshot::Sender<bool>,
// }

/// Represents an Action handled by a plugin on reaction core's side.
///
/// During [`PluginInfo::load_config`], the plugin should create a [`remoc::rch::mpsc::channel`] of [`Exec`].
/// It will keep the receiving side for itself and put the sending side in a [`ActionImpl`].
///
/// The plugin will start receiving [`Exec`]s in the channel from reaction only after [`PluginInfo::start`] has been called by reaction core.
#[derive(Clone, Serialize, Deserialize)]
pub struct ActionImpl {
    pub tx: rch::mpsc::Sender<Exec>,
}

/// A [trigger](https://reaction.ppom.me/reference.html#trigger) of the Action, sent by reaction core to the plugin.
///
/// The plugin should perform the configured action for each received [`Exec`].
///
/// Any error during its execution should be logged to stderr, see [`crate#Errors`] for error handling recommandations.
#[derive(Serialize, Deserialize)]
pub struct Exec {
    pub match_: Vec<String>,
    pub time: Duration,
}

/// The main loop for a plugin.
///
/// Bootstraps the communication with reaction core on the process' stdin and stdout,
/// then holds the connection and maintains the plugin in a server state.
///
/// Your main function should only create a struct that implements [`PluginInfo`]
/// and then call [`main_loop`]:
/// ```ignore
/// #[tokio::main]
/// async fn main() {
///     let plugin = MyPlugin::default();
///     reaction_plugin::main_loop(plugin).await;
/// }
/// ```
pub async fn main_loop<T: PluginInfo + Send + Sync + 'static>(plugin_info: T) {
    // First check that we're called by reaction
    let mut args = args();
    // skip 0th argument
    let _skip = args.next();
    if args.next().is_none_or(|arg| arg != "serve") {
        eprintln!("This plugin is not meant to be called as-is.");
        eprintln!(
            "reaction daemon starts plugins itself and communicates with them on stdin, stdout and stderr."
        );
        eprintln!("See the doc on plugin configuration: https://reaction.ppom.me/plugins/");
        exit(1);
    } else {
        let (conn, mut tx, _rx): (
            _,
            remoc::rch::base::Sender<PluginInfoClient>,
            remoc::rch::base::Receiver<()>,
        ) = Connect::io(remoc::Cfg::default(), stdin(), stdout())
            .await
            .unwrap();

        let (server, client) = PluginInfoServer::new(plugin_info, 1);

        let (res1, (_, res2), res3) = tokio::join!(tx.send(client), server.serve(), conn);
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
            eprintln!("ERROR connection error with reaction: {err}");
            exit_code = 3;
        }
        exit(exit_code);
    }
}

// Errors

pub type RemoteResult<T> = Result<T, RemoteError>;

/// reaction-plugin's Error type.
#[derive(Debug, Serialize, Deserialize)]
pub enum RemoteError {
    /// A connection error that origins from [`remoc`], the crate used for communication on the plugin's `stdin`/`stdout`.
    ///
    /// You should not instantiate this type of error yourself.
    Remoc(rtc::CallError),
    /// A free String for application-specific errors.
    ///
    /// You should only instantiate this type of error yourself, for any error that you encounter at startup and shutdown.
    ///
    /// Otherwise, any error during the plugin's runtime should be logged to stderr, see [`crate#Errors`] for error handling recommandations.
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
