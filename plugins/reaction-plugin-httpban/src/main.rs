use std::collections::BTreeSet;

use async_trait::async_trait;
use pingora_core::server::{
    ExecutionPhase, RunArgs, Server, ShutdownSignal, ShutdownSignalWatch, UnixShutdownSignalWatch,
};
use reaction_plugin::{
    ActionConfig, ActionImpl, Hello, Manifest, PluginInfo, RemoteResult, StreamConfig, StreamImpl,
    Value,
};
use remoc::rtc;
use tokio::sync::oneshot::{self};
use tokio_util::sync::CancellationToken;

use crate::proxy::Proxy;

mod action;
mod config;
mod error;
mod proxy;
mod request;
mod resolve_location;
#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() {
    let plugin = MyPlugin::default();
    reaction_plugin::main_loop(plugin).await;
}

#[derive(Default)]
pub struct MyPlugin {
    server: Option<Server>,
    token: CancellationToken,
    has_quit: Option<oneshot::Receiver<()>>,
}

impl PluginInfo for MyPlugin {
    async fn manifest(&mut self) -> Result<Manifest, rtc::CallError> {
        Ok(Manifest {
            hello: Hello::new(),
            streams: BTreeSet::default(),
            actions: BTreeSet::from(["httpban".into()]),
        })
    }

    async fn load_config(
        &mut self,
        streams: Vec<StreamConfig>,
        actions: Vec<ActionConfig>,
        options: Vec<Value>,
    ) -> RemoteResult<(Vec<StreamImpl>, Vec<ActionImpl>)> {
        let (computed_main_options, computed_action_options, ret_actions) =
            config::load_config(streams, actions, options).await?;

        for action in computed_action_options {
            tokio::spawn(async move { action.serve().await });
        }

        let server = Proxy::new(computed_main_options);
        self.server = Some(server);

        Ok((vec![], ret_actions))
    }

    async fn start(&mut self) -> RemoteResult<()> {
        let (token, shutdown) = Shutdown::new();
        self.token = token;

        let server = self.server.take().ok_or_else(|| {
            format!("plugin httpban has not been initialized. this is a bug from reaction!")
        })?;

        let mut phases = server.watch_execution_phase();
        let (tx, rx) = oneshot::channel();
        self.has_quit = Some(rx);
        tokio::spawn(async move {
            while let Ok(phase) = phases.recv().await {
                if let ExecutionPhase::Terminated = phase {
                    let _ = tx.send(());
                    break;
                }
            }
        });

        tokio::task::spawn_blocking(|| {
            server.run(RunArgs {
                shutdown_signal: Box::new(shutdown),
            })
        });
        Ok(())
    }

    async fn close(self) -> RemoteResult<()> {
        self.token.cancel();
        let _ = self.has_quit.unwrap().await;
        Ok(())
    }
}

pub struct Shutdown {
    token: CancellationToken,
    unix: UnixShutdownSignalWatch,
}
impl Shutdown {
    pub fn new() -> (CancellationToken, Self) {
        let token = CancellationToken::new();
        let unix = UnixShutdownSignalWatch;
        (token.clone(), Self { token, unix })
    }
}
#[async_trait]
impl ShutdownSignalWatch for Shutdown {
    async fn recv(&self) -> ShutdownSignal {
        let cancel = self.token.clone().cancelled_owned();
        tokio::select! {
            _ = cancel => ShutdownSignal::GracefulTerminate,
            ret = self.unix.recv() => ret,
        }
    }
}
