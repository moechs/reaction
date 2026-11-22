//! reaction-plugin-container: feed reaction streams with container logs.
//!
//! Streams select containers by name or label through the Docker Engine API.

// Without any backend feature the core is compiled but unused.
#![cfg_attr(not(feature = "docker"), allow(dead_code, unused))]

#[macro_use]
mod log;

mod backend;
mod core;
mod plugin;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() {
    reaction_plugin::main_loop(plugin::Plugin::default()).await;
}
