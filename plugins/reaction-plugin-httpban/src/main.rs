use std::collections::{BTreeMap, BTreeSet};

use reaction_plugin::{
    ActionConfig, ActionImpl, Hello, Manifest, PluginInfo, RemoteResult, StreamConfig, StreamImpl,
};
use remoc::rtc;

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
    ) -> RemoteResult<(Vec<StreamImpl>, Vec<ActionImpl>)> {
        if !streams.is_empty() {
            return Err("This plugin can't handle any stream type".into());
        }

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
