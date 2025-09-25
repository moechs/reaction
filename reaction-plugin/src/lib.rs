/// This crate permits to define a stable ABI between reaction's core and plugins.
///
/// It uses [`stabby`], that rewrites essential std types as stable repr("C") types,
/// and permits to check a lof of safeness related issues at compile time and at runtime, when
/// loading a library.
///
/// To implement a plugin, one has to provide an implementation of [`PluginInfo`], that provides
/// the entrypoint for a plugin.
/// It permits to define 0 to n (stream, filter, action) custom types.
use stabby::{
    boxed::Box, future::DynFuture, option::Option, result::Result, string::String, vec::Vec,
};

mod value;
pub use value::Value;

/// This is the only trait that **must** be implemented by a plugin.
/// It provides lists of stream, filter and action types implemented by a dynamic plugin.
#[stabby::stabby(checked)]
pub trait PluginInfo {
    /// Return all stream types that should be made available to reaction users
    extern "C" fn stream_impls(&self) -> Vec<String>;
    /// Return one instance of a given type.
    extern "C" fn stream_impl(
        &mut self,
        stream_name: String,
        stream_type: String,
        config: Value,
    ) -> Result<BoxedStreamImpl, PluginError>;

    /// Return all filter types that should be made available to reaction users
    extern "C" fn filter_impls(&self) -> Vec<String>;
    /// Return one instance of a given type.
    extern "C" fn filter_impl(
        &mut self,
        stream_name: String,
        filter_name: String,
        filter_type: String,
        config: Value,
    ) -> Result<BoxedFilterImpl, PluginError>;

    /// Return all action types that should be made available to reaction users
    extern "C" fn action_impls(&self) -> Vec<String>;
    /// Return one instance of a given type.
    extern "C" fn action_impl(
        &mut self,
        stream_name: String,
        filter_name: String,
        action_name: String,
        action_type: String,
        config: Value,
    ) -> Result<BoxedActionImpl, PluginError>;

    /// Notify the plugin that setup is finished, permitting a last occasion to report an error
    /// (For example if a stream wants a companion action but it hasn't been initialized)
    extern "C" fn finish_setup(&mut self) -> Result<(), String>;
}

pub enum PluginError {
    InexistantType,
    InitialisationError(String),
}

type BoxedStreamImpl = stabby::dynptr!(Box<dyn StreamImpl>);
type BoxedFilterImpl = stabby::dynptr!(Box<dyn FilterImpl>);
type BoxedActionImpl = stabby::dynptr!(Box<dyn ActionImpl>);

#[stabby::stabby(checked)]
pub trait StreamImpl {
    extern "C" fn next<'a>(&'a mut self) -> DynFuture<'a, Result<Option<Vec<u8>>, String>>;
    extern "C" fn close<'a>(&'a mut self) -> DynFuture<'a, Result<(), String>>;
}

#[stabby::stabby(checked)]
pub trait FilterImpl {
    extern "C" fn matches(&mut self, line: String) -> bool;
    extern "C" fn close(&mut self) -> Result<(), String>;
}

#[stabby::stabby(checked)]
pub trait ActionImpl {
    extern "C" fn next<'a>(&'a mut self) -> DynFuture<'a, Result<Option<Vec<u8>>, String>>;
    extern "C" fn close<'a>(&'a mut self) -> DynFuture<'a, Result<(), String>>;
}
