use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
};

use reaction_plugin::{ActionImpl, PluginInfo, PluginInfoClient, StreamImpl};
use remoc::Connect;
use serde_json::Value;
use tokio::process::Child;

use crate::concepts::Plugin;

mod value;

use value::to_stable_value;

pub struct PluginManager {
    child: Child,
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
    async fn new(plugin: &'static Plugin) -> Result<Self, String> {
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
            .map_err(|err| format!("could not init communication with plugin: {err}"))?;

        tokio::spawn(conn);

        let plugin_info = rx
            .recv()
            .await
            .map_err(|err| format!("could not retrieve initial information from plugin: {err}"))?
            .ok_or("could not retrieve initial information from plugin: no data")?;

        Ok(Self {
            child,
            plugin,
            plugin_info,
        })
    }
}

#[derive(Default)]
pub struct Plugins {
    plugins: BTreeMap<String, PluginManager>,
    streams: BTreeMap<String, String>,
    actions: BTreeMap<String, String>,
}

impl Plugins {
    pub async fn new(plugins: &'static Vec<Plugin>) -> Result<Self, String> {
        let mut this = Self::default();

        for plugin in plugins {
            let path = plugin.path.clone();
            this.load_plugin(&plugin)
                .await
                .map_err(|err| format!("plugin {path}: {err}]"))?;
        }

        Ok(this)
    }

    async fn load_plugin(&mut self, plugin: &'static Plugin) -> Result<(), String> {
        let path = plugin.path.clone();
        let manager = PluginManager::new(plugin).await?;

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

    pub async fn finish_plugin_setup(self) -> Result<(), String> {
        for mut plugin in self.plugins.into_values() {
            plugin
                .finish_setup()
                .await
                .map_err(|err| format!("plugin error: {err}"))?
                .map_err(|err| format!("invalid config for plugin: {err}"))?;
        }
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
}
