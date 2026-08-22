use std::collections::BTreeSet;

use reaction_plugin::{
    ActionConfig, ActionImpl, Hello, Manifest, PluginInfo, RemoteResult, StreamConfig, StreamImpl,
    Value,
};
use remoc::rtc;

use crate::config::{ActionOptions, MainOptions};

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

        let computed_main_options = options.setup()?;

        let mut computed_action_options = Vec::new();

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

            let options: ActionOptions = serde_json::from_value(config.into())
                .map_err(|err| format!("invalid options for action {name}: {err}"))?;

            let computed = options
                .setup()
                .map_err(|err| format!("invalid action {name}: {err}"))?;

            computed_action_options.push(computed);
        }

        // check that all sets referenced by domains or by actions are used by the each others.
        {
            let domain_sets: BTreeSet<&String> = computed_main_options
                .domains
                .values()
                .flat_map(|domain| domain.cases.keys())
                .collect();

            let action_sets: BTreeSet<&String> = computed_action_options
                .iter()
                .map(|action| &action.set)
                .collect();

            let mut errors = Vec::new();
            errors.extend(domain_sets.difference(&action_sets).map(|set|
                format!("Set {set} is referenced by a domain, but is not referenced in any action, so it would stay empty forever and never match")
            ));
            errors.extend(action_sets.difference(&domain_sets).map(|set| format!("Set {set} is referenced by an action, but is not used in any domain, so it would never be actually used")));
            if !errors.is_empty() {
                return Err(errors.join("\n").into());
            }
        }

        // TODO construct and return ActionImpls

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
