use std::{
    collections::{BTreeMap, BTreeSet},
    ops::{Deref, DerefMut},
    time::Duration,
};

use futures::{future::join_all, FutureExt};
use reaction_plugin::{ActionImpl, Hello, PluginInfo, PluginInfoClient, StreamImpl};
use remoc::Connect;
use serde_json::Value;
use tokio::{process::Child, time::sleep};
use tracing::error;

use crate::{
    concepts::{Config, Plugin},
    daemon::{utils::kill_child, ShutdownToken},
};

pub struct PluginManager {
    child: Child,
    shutdown: ShutdownToken,
    plugin: &'static Plugin,
    plugin_info: PluginInfoClient,
    streams: BTreeSet<String>,
    actions: BTreeSet<String>,
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
    async fn new(
        plugin: &'static Plugin,
        state_directory: &str,
        shutdown: ShutdownToken,
    ) -> Result<Self, String> {
        let mut child = plugin
            .launch(state_directory)
            .await
            .map_err(|err| format!("could not launch plugin: {err}"))?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let (conn, _tx, mut rx): (
            _,
            remoc::rch::base::Sender<()>,
            remoc::rch::base::Receiver<PluginInfoClient>,
        ) = Connect::io(remoc::Cfg::default(), stdout, stdin)
            .await
            .map_err(|err| {
                format!(
                    "could not init communication with plugin {}: {err}",
                    plugin.name
                )
            })?;

        tokio::spawn(conn);

        let mut plugin_info = rx
            .recv()
            .await
            .map_err(|err| format!("could not retrieve initial information from plugin: {err}"))?
            .ok_or("could not retrieve initial information from plugin: no data")?;

        let manifest = plugin_info
            .manifest()
            .await
            .map_err(|err| format!("error while getting plugin {} manifest: {err}", plugin.name))?;

        let my_hello = Hello::hello();

        if let Err(hint) = Hello::is_compatible(&my_hello, &manifest.hello) {
            return Err(format!(
                "reaction can't handle plugin {} with incompatible version {}.{}: current version: {}.{}. {}",
                plugin.name,
                manifest.hello.version_major, manifest.hello.version_minor,
                my_hello.version_major, my_hello.version_minor,
                hint
            ));
        }

        Ok(Self {
            child,
            shutdown,
            plugin,
            plugin_info,
            streams: manifest.streams,
            actions: manifest.actions,
        })
    }

    async fn handle_child(mut self) {
        const PLUGIN_STOP_GRACE_TIME: u64 = 15;

        // wait either for the child process to exit on its own or for the shutdown signal
        futures::select! {
            _ = self.child.wait().fuse() => {
                error!("plugin {} exited: its command returned.", self.plugin.name);
                return;
            }
            _ = self.shutdown.wait().fuse() => {}
        }

        futures::select! {
        _ = self.plugin_info.close().fuse() => {
                return;
            },
            _ = sleep(Duration::from_secs(PLUGIN_STOP_GRACE_TIME)).fuse() => {
                error!("plugin {} did not respond to close request in time, killing", self.plugin.name)
            },
        }

        kill_child(self.child, format!("plugin {}", self.plugin.name), 5).await;
    }
}

#[derive(Default)]
pub struct Plugins {
    plugins: BTreeMap<String, PluginManager>,
    streams: BTreeMap<String, String>,
    actions: BTreeMap<String, String>,
}

impl Plugins {
    pub async fn new(config: &'static Config, shutdown: ShutdownToken) -> Result<Self, String> {
        let mut this = Self::default();

        for plugin in config.plugins.values() {
            let name = plugin.name.clone();
            this.load_plugin(&plugin, &config.state_directory, shutdown.clone())
                .await
                .map_err(|err| format!("plugin {name}: {err}]"))?;
        }

        Ok(this)
    }

    async fn load_plugin(
        &mut self,
        plugin: &'static Plugin,
        state_directory: &str,
        shutdown: ShutdownToken,
    ) -> Result<(), String> {
        let name = plugin.name.clone();
        let manager = PluginManager::new(plugin, state_directory, shutdown).await?;

        for stream in &manager.streams {
            if let Some(name) = self.streams.insert(stream.clone().into(), name.clone()) {
                return Err(format!(
                    "plugin {name} already exposed a stream with type name '{stream}'",
                ));
            }
        }

        for action in &manager.actions {
            if let Some(name) = self.actions.insert(action.clone().into(), name.clone()) {
                return Err(format!(
                    "plugin {name} already exposed a action with type name '{action}'",
                ));
            }
        }

        self.plugins.insert(name, manager);
        Ok(())
    }

    pub async fn init_stream_impl(
        &mut self,
        stream_name: String,
        stream_type: String,
        config: Value,
    ) -> Result<StreamImpl, String> {
        let plugin_name = self.streams.get(&stream_type).ok_or(format!(
            "No plugin provided the stream type '{stream_type}'"
        ))?;

        let plugin = self.plugins.get_mut(plugin_name).unwrap();

        plugin
            .stream_impl(stream_name.clone(), stream_type, config)
            .await
            .map_err(|err| format!("plugin error while initializing stream {stream_name}: {err}"))
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
        let plugin_name = self.actions.get(&action_type).ok_or(format!(
            "No plugin provided the action type '{action_type}'"
        ))?;

        let plugin = self.plugins.get_mut(plugin_name).unwrap();

        plugin
            .action_impl(
                stream_name.clone(),
                filter_name.clone(),
                action_name.clone(),
                action_type,
                config,
                patterns,
            )
            .await
            .map_err(|err| format!("plugin error while initializing action {stream_name}.{filter_name}.{action_name}: {err}"))
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
            result.map_err(|err| format!("plugin {} error: {err}", plugin_manager.plugin.name))
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
