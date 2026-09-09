use std::{collections::BTreeSet, time::Duration};

use async_trait::async_trait;
use pingora_core::server::{
    RunArgs, Server, ShutdownSignal, ShutdownSignalWatch, UnixShutdownSignalWatch,
};
use reaction_plugin::{
    ActionConfig, ActionImpl, Hello, Manifest, PluginInfo, RemoteResult, StreamConfig, StreamImpl,
    Value,
};
use remoc::rtc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::proxy::Proxy;

mod action;
mod config;
mod proxy;
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

        let server = Proxy::new(computed_main_options, Some(2), Some(2));
        self.server = Some(server);

        Ok((vec![], ret_actions))
    }

    async fn start(&mut self) -> RemoteResult<()> {
        let (token, shutdown) = Shutdown::new();
        self.token = token;

        self.server
            .take()
            .ok_or_else(|| format!("plugin has not been initialized. this is a bug!"))?
            .run(RunArgs {
                shutdown_signal: Box::new(shutdown),
            });
        Ok(())
    }

    async fn close(self) -> RemoteResult<()> {
        self.token.cancel();
        // FIXME hook on the cancellation instead of sleeping 4 seconds
        sleep(Duration::from_secs(4)).await;
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
