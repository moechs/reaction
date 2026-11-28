//! reaction-plugin-syslog: feed reaction streams with syslog messages.

#[macro_use]
mod log;

mod backend;
mod config;
mod dispatch;
mod lines;
mod message;
mod plugin;
mod template;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() {
    reaction_plugin::main_loop(plugin::Plugin::default()).await;
}
