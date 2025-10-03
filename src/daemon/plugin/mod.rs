use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    time::Duration,
};

use futures::{future::join_all, FutureExt};
use reaction_plugin::{ActionImpl, PluginInfo, PluginInfoClient, StreamImpl};
use remoc::Connect;
use serde_json::Value;
use tokio::{process::Child, time::sleep};
use tracing::error;

use crate::{
    concepts::Plugin,
    daemon::{utils::kill_child, ShutdownToken},
};

mod value;

use value::to_stable_value;

pub struct PluginManager {
    child: Child,
    shutdown: ShutdownToken,
    plugin: &'static Plugin,
    plugin_info: PluginInfoClient,
}

impl Deref for PluginManager {
    type Target = PluginInfoClient;
    fn deref(&self) -> &Self::Target {
        &self.plugin_info
    }
}

impl DerefMut for PluginManager {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.plugin_info
    }
}

impl PluginManager {
    async fn new(plugin: &'static Plugin, shutdown: ShutdownToken) -> Result<Self, String> {
        let mut child = plugin
            .launch()
            .map_err(|err| format!("could not launch plugin: {err}"))?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let (conn, _tx, mut rx): (
            _,
            remoc::rch::base::Sender<()>,
            remoc::rch::base::Receiver<PluginInfoClient>,
        ) = Connect::io_buffered(remoc::Cfg::default(), stdout, stdin, 2048)
            .await
            .map_err(|err| {
                format!(
                    "could not init communication with plugin {}: {err}",
                    plugin.path
                )
            })?;

        tokio::spawn(conn);

        let plugin_info = rx
            .recv()
            .await
            .map_err(|err| format!("could not retrieve initial information from plugin: {err}"))?
            .ok_or("could not retrieve initial information from plugin: no data")?;

        Ok(Self {
            child,
            shutdown,
            plugin,
            plugin_info,
        })
    }

    async fn handle_child(mut self) {
        const PLUGIN_STOP_GRACE_TIME: u64 = 15;

        // wait either for the child process to exit on its own or for the shutdown signal
        futures::select! {
            _ = self.child.wait().fuse() => {
                error!("plugin {} exited: its command returned.", self.plugin.path);
                return;
            }
            _ = self.shutdown.wait().fuse() => {}
        }

        futures::select! {
        _ = self.plugin_info.close().fuse() => {
                return;
            },
            _ = sleep(Duration::from_secs(PLUGIN_STOP_GRACE_TIME)).fuse() => {
                error!("plugin {} did not respond to close request in time, killing", self.plugin.path)
            },
        }

        kill_child(self.child, format!("plugin {}", self.plugin.path), 5).await;
    }
}

#[derive(Default)]
pub struct Plugins {
    plugins: BTreeMap<String, PluginManager>,
    streams: BTreeMap<String, String>,
    actions: BTreeMap<String, String>,
}

impl Plugins {
    pub async fn new(
        plugins: &'static Vec<Plugin>,
        shutdown: ShutdownToken,
    ) -> Result<Self, String> {
        let mut this = Self::default();

        for plugin in plugins {
            let path = plugin.path.clone();
            this.load_plugin(&plugin, shutdown.clone())
                .await
                .map_err(|err| format!("plugin {path}: {err}]"))?;
        }

        Ok(this)
    }

    async fn load_plugin(
        &mut self,
        plugin: &'static Plugin,
        shutdown: ShutdownToken,
    ) -> Result<(), String> {
        let path = plugin.path.clone();
        let manager = PluginManager::new(plugin, shutdown).await?;

        for stream in manager
            .stream_impls()
            .await
            .map_err(|err| format!("plugin error: {err}"))?
        {
            if let Some(path) = self.streams.insert(stream.clone().into(), path.clone()) {
                return Err(format!(
                    "plugin {path} already exposed a stream with type name '{stream}'"
                ));
            }
        }

        for action in manager
            .action_impls()
            .await
            .map_err(|err| format!("plugin error: {err}"))?
        {
            if let Some(path) = self.actions.insert(action.clone().into(), path.clone()) {
                return Err(format!(
                    "plugin {path} already exposed a action with type name '{action}'"
                ));
            }
        }

        self.plugins.insert(path, manager);
        Ok(())
    }

    pub async fn init_stream_impl(
        &mut self,
        stream_name: String,
        stream_type: String,
        config: Value,
    ) -> Result<StreamImpl, String> {
        let plugin_name = self
            .streams
            .get(&stream_type)
            .ok_or(format!("No plugin provided a stream type '{stream_type}'"))?;

        let plugin = self.plugins.get_mut(plugin_name).unwrap();

        plugin
            .stream_impl(
                stream_name.into(),
                stream_type.into(),
                to_stable_value(config),
            )
            .await
            .map_err(|err| format!("plugin error: {err}"))?
    }

    pub async fn init_action_impl(
        &mut self,
        stream_name: String,
        filter_name: String,
        action_name: String,
        action_type: String,
        config: Value,
        patterns: Vec<String>,
    ) -> Result<ActionImpl, String> {
        let plugin_name = self
            .actions
            .get(&action_type)
            .ok_or(format!("No plugin provided a action type '{action_type}'"))?;

        let plugin = self.plugins.get_mut(plugin_name).unwrap();

        plugin
            .action_impl(
                stream_name.into(),
                filter_name.into(),
                action_name.into(),
                action_type.into(),
                to_stable_value(config),
            )
            .await
            .map_err(|err| format!("plugin error: {err}"))?
    }

    pub async fn finish_setup(&mut self) -> Result<(), String> {
        // Finish setup of all plugins
        join_all(
            self.plugins
                .values_mut()
                .map(|plugin_manager| plugin_manager.finish_setup()),
        )
        .await
        // Convert Vec<Result<Result>> into Result
        .into_iter()
        .zip(self.plugins.values())
        .map(|(result, plugin_manager)| {
            result
                .map_err(|err| format!("plugin {} error: {err}", plugin_manager.plugin.path))
                .map_err(|err| {
                    format!(
                        "invalid config for plugin {}: {err}",
                        plugin_manager.plugin.path
                    )
                })
                .flatten()
        })
        .collect::<Result<(), String>>()
    }

    pub fn manager(self) {
        for plugin in self.plugins.into_values() {
            tokio::spawn(async move {
                plugin.handle_child().await;
            });
        }
    }
}
