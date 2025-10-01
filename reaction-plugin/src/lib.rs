/// This crate defines the API between reaction's core and plugins.
///
/// It's based on [`remoc`], which permits to multiplex channels and remote objects/functions/trait
/// calls over a single transport channel.
///
/// To implement a plugin, one has to provide an implementation of [`PluginInfo`], that provides
/// the entrypoint for a plugin.
/// It permits to define 0 to n (stream, filter, action) custom types.
use remoc::{rch, rtc};
use serde::{Deserialize, Serialize};

/// This is the only trait that **must** be implemented by a plugin.
/// It provides lists of stream, filter and action types implemented by a dynamic plugin.
#[rtc::remote]
pub trait PluginInfo {
    /// Return all stream types that should be made available to reaction users
    async fn stream_impls(&self) -> RemoteResult<Vec<String>>;
    /// Return one stream of a given type if it exists
    async fn stream_impl(
        &mut self,
        stream_name: String,
        stream_type: String,
        config: Value,
    ) -> RemoteResult<Result<StreamImpl, String>>;

    // /// Return all filter types that should be made available to reaction users
    // async fn filter_impls(&self) -> RemoteResult<Vec<String>>;
    // /// Return one stream of a given type if it exists
    // async fn filter_impl(
    //     &mut self,
    //     stream_name: String,
    //     filter_name: String,
    //     stream_type: String,
    //     config: Value,
    // ) -> RemoteResult<Result<FilterImpl, String>>;

    /// Return all action types that should be made available to reaction users
    async fn action_impls(&self) -> RemoteResult<Vec<String>>;
    /// Return one instance of a given type.
    async fn action_impl(
        &mut self,
        stream_name: String,
        filter_name: String,
        action_name: String,
        action_type: String,
        config: Value,
    ) -> RemoteResult<Result<ActionImpl, String>>;

    /// Notify the plugin that setup is finished, permitting a last occasion to report an error
    /// (For example if a stream wants a companion action but it hasn't been initialized)
    async fn finish_setup(&mut self) -> RemoteResult<Result<(), String>>;
}

pub type RemoteResult<T> = Result<T, rtc::CallError>;

/// Represents a configuration value.
/// This is not meant as an efficient type, but as a very flexible one.
#[derive(Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

#[derive(Serialize, Deserialize)]
pub struct StreamImpl {
    pub stream: rch::lr::Receiver<Result<String, String>>,
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

#[derive(Serialize, Deserialize)]
pub struct ActionImpl {
    pub stream: rch::lr::Sender<Exec>,
}

#[derive(Serialize, Deserialize)]
pub struct Exec {
    pub match_: Vec<String>,
    pub result: rch::oneshot::Sender<String>,
}
