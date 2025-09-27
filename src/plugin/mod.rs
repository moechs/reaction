use std::{collections::BTreeMap, path::PathBuf};

use reaction_plugin::{
    BoxedActionImpl, BoxedPluginInfo, BoxedStreamImpl, PluginInfoDyn, PluginInfoDynMut,
};
use serde_json::Value;
use stabby::libloading::StabbyLibrary;
use tokio::{fs::read_dir, runtime::Handle};
use value::to_stable_value;

mod value;

#[derive(Default)]
pub struct Plugins {
    plugins: BTreeMap<String, BoxedPluginInfo>,
    streams: BTreeMap<String, String>,
    // filters: BTreeMap<String, String>,
    actions: BTreeMap<String, String>,
}

impl Plugins {
    pub async fn import(&mut self, plugin_directories: Vec<String>) -> Result<(), String> {
        for plugin_directory in plugin_directories {
            let mut dir_entries = read_dir(&plugin_directory).await.map_err(|err| {
                format!("Error reading plugin directory {plugin_directory}: {err}")
            })?;
            loop {
                match dir_entries.next_entry().await {
                    Err(err) => {
                        return Err(format!(
                            "Error reading plugin directory {plugin_directory}: {err}"
                        ))
                    }

                    Ok(None) => break,

                    Ok(Some(entry)) => {
                        let filename = PathBuf::from(&plugin_directory).join(&entry.file_name());
                        self.load_plugin(filename.clone())
                            .await
                            .map_err(|err| format!("Error loading plugin {filename:?}: {err}"))?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn load_plugin(&mut self, filename: PathBuf) -> Result<(), String> {
        // TODO check ownership of file?

        let name = filename.to_string_lossy().to_string();
        // SAFETY This function is exposed by libloading as unsafe
        // But we're (hopefully) gonna be safe with stabby <3
        #[allow(unsafe_code)]
        let plugin = Handle::current()
            .spawn_blocking(|| unsafe { libloading::Library::new(filename) })
            .await
            // Join Error
            .map_err(|err| err.to_string())?
            // Libloading Error
            .map_err(|err| err.to_string())?;

        // SAFETY This function is exposed by stabby as unsafe
        // But we're (hopefully) gonna be safe <3
        #[allow(unsafe_code)]
        let plugin_init = unsafe {
            plugin.get_stabbied::<extern "C" fn() -> BoxedPluginInfo>(b"reaction_plugin")
        }.map_err(|err| format!("expected entrypoint `fn reaction_plugin() -> BoxedPluginInfo` is either not present or malformed: {err}"))?;

        let plugin_info = plugin_init();

        for stream in plugin_info.stream_impls() {
            if let Some(name) = self.streams.insert(stream.clone().into(), name.clone()) {
                return Err(format!(
                    "plugin {name} already exposed a stream with type name '{stream}'"
                ));
            }
        }

        // for filter in plugin_info.filter_impls() {
        //     if let Some(name) = self.filters.insert(filter.clone().into(), name.clone()) {
        //         return Err(format!(
        //             "plugin {name} already exposed a filter with type name '{filter}'"
        //         ));
        //     }
        // }

        for action in plugin_info.action_impls() {
            if let Some(name) = self.actions.insert(action.clone().into(), name.clone()) {
                return Err(format!(
                    "plugin {name} already exposed a action with type name '{action}'"
                ));
            }
        }

        self.plugins.insert(name, plugin_info);

        Ok(())
    }

    pub fn finish_plugin_setup(self) -> Result<(), String> {
        for mut plugin in self.plugins.into_values() {
            // Didn't find a more elegant way to manipulate [`stabby::result::Result`]
            let result = plugin.finish_setup();
            if result.is_err() {
                return Err(result.unwrap_err().into());
            }
        }
        Ok(())
    }

    pub fn init_stream_impl(
        &mut self,
        stream_name: String,
        stream_type: String,
        config: Value,
    ) -> Result<BoxedStreamImpl, String> {
        let plugin_name = self
            .streams
            .get(&stream_type)
            .ok_or(format!("No plugin provided a stream type '{stream_type}'"))?;

        let plugin = self.plugins.get_mut(plugin_name).unwrap();

        let result = plugin.stream_impl(
            stream_name.into(),
            stream_type.into(),
            to_stable_value(config),
        );

        if result.is_ok() {
            Ok(result.unwrap())
        } else {
            Err(result.err().unwrap().into())
        }
    }

    pub fn init_action_impl(
        &mut self,
        stream_name: String,
        filter_name: String,
        action_name: String,
        action_type: String,
        config: Value,
    ) -> Result<BoxedActionImpl, String> {
        let plugin_name = self
            .actions
            .get(&action_type)
            .ok_or(format!("No plugin provided a action type '{action_type}'"))?;

        let plugin = self.plugins.get_mut(plugin_name).unwrap();

        let result = plugin.action_impl(
            stream_name.into(),
            filter_name.into(),
            action_name.into(),
            action_type.into(),
            to_stable_value(config),
        );

        if result.is_ok() {
            Ok(result.unwrap())
        } else {
            Err(result.err().unwrap().into())
        }
    }
}
