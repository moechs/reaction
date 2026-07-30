use std::collections::BTreeSet;

use reaction_plugin::{
    ActionConfig, ActionImpl, Hello, Manifest, PluginInfo, RemoteResult, StreamConfig, StreamImpl,
    Value,
};
use remoc::rtc;

use crate::config::MainOptions;

mod config;

#[tokio::main]
async fn main() {
    let plugin = MyPlugin::default();
    reaction_plugin::main_loop(plugin).await;
}

#[derive(Default)]
struct MyPlugin {}

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
        if !streams.is_empty() {
            return Err("This plugin can't handle any stream type".into());
        }

        let options: MainOptions =
            serde_json::from_value(options.into()).map_err(|err| format!("{err}"))?;

        let computed_options = options.setup()?;

        for ActionConfig {
            stream_name,
            filter_name,
            action_name,
            action_type,
            config,
            patterns,
        } in actions
        {
            if &action_type != "httpban" {
                return Err("This plugin can't handle other action types than httpban".into());
            }

            let name = format!("{stream_name}.{filter_name}.{action_name}");
        }

        // TODO check that all sets referenced by domains or by actions are used by the each others.

        Ok((vec![], vec![]))
    }

    async fn start(&mut self) -> RemoteResult<()> {
        todo!()
    }

    async fn close(self) -> RemoteResult<()> {
        todo!()
    }
    // ...
}
