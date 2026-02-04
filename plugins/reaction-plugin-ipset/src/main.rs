use std::collections::{BTreeMap, BTreeSet};

use reaction_plugin::{
    ActionImpl, Hello, Manifest, PluginInfo, RemoteError, RemoteResult, StreamImpl, Value,
    shutdown::{ShutdownController, ShutdownToken},
};
use remoc::rtc;

use crate::{
    action::{Action, ActionOptions, Set, SetOptions},
    ipset::IpSet,
};

#[cfg(test)]
mod tests;

mod action;
mod ipset;

#[tokio::main]
async fn main() {
    let plugin = Plugin::default();
    reaction_plugin::main_loop(plugin).await;
}

#[derive(Default)]
struct Plugin {
    ipset: IpSet,
    set_options: BTreeMap<String, SetOptions>,
    sets: Vec<Set>,
    actions: Vec<Action>,
    shutdown: ShutdownController,
}

impl PluginInfo for Plugin {
    async fn manifest(&mut self) -> Result<Manifest, rtc::CallError> {
        Ok(Manifest {
            hello: Hello::new(),
            streams: BTreeSet::default(),
            actions: BTreeSet::from(["ipset".into()]),
        })
    }

    async fn stream_impl(
        &mut self,
        _stream_name: String,
        _stream_type: String,
        _config: Value,
    ) -> RemoteResult<StreamImpl> {
        Err("This plugin can't handle any stream type".into())
    }

    async fn action_impl(
        &mut self,
        stream_name: String,
        filter_name: String,
        action_name: String,
        action_type: String,
        config: Value,
        patterns: Vec<String>,
    ) -> RemoteResult<ActionImpl> {
        if &action_type != "ipset" {
            return Err("This plugin can't handle other action types than ipset".into());
        }

        let mut options: ActionOptions = serde_json::from_value(config.into()).map_err(|err| {
            format!("invalid options for action {stream_name}.{filter_name}.{action_name}: {err}")
        })?;

        options.set_ip_index(patterns).map_err(|_|
                format!(
                    "No pattern with name {} in filter {stream_name}.{filter_name}. Try setting the option `pattern` to your pattern name of type 'ip'",
                    &options.pattern
                )
        )?;

        // Merge option
        self.set_options
            .entry(options.set.clone())
            .or_default()
            .merge(&options.set_options)
            .map_err(|err| format!("ipset {}: {err}", options.set))?;

        let (tx, rx) = remoc::rch::mpsc::channel(1);
        self.actions.push(Action::new(
            self.ipset.clone(),
            self.shutdown.token(),
            rx,
            options,
        )?);

        Ok(ActionImpl { tx })
    }

    async fn finish_setup(&mut self) -> RemoteResult<()> {
        // Init all sets
        while let Some((name, options)) = self.set_options.pop_first() {
            self.sets.push(Set::from(name, options));
        }
        self.set_options = Default::default();

        let mut first_error = None;
        for (i, set) in self.sets.iter().enumerate() {
            // Retain if error
            if let Err(err) = set.init(&mut self.ipset).await {
                first_error = Some((i, RemoteError::Plugin(err)));
                break;
            }
        }
        // Destroy initialized sets if error
        if let Some((i, err)) = first_error {
            for set in self.sets.iter().take(i + 1) {
                let _ = set.destroy(&mut self.ipset).await;
            }
            return Err(err);
        }

        // Launch a task that will destroy the sets on shutdown
        tokio::spawn(destroy_sets_at_shutdown(
            self.ipset.clone(),
            std::mem::take(&mut self.sets),
            self.shutdown.token(),
        ));

        // Launch all actions
        while let Some(action) = self.actions.pop() {
            tokio::spawn(async move { action.serve().await });
        }
        self.actions = Default::default();

        Ok(())
    }

    async fn close(self) -> RemoteResult<()> {
        self.shutdown.ask_shutdown();
        self.shutdown.wait_all_task_shutdown().await;
        Ok(())
    }
}

async fn destroy_sets_at_shutdown(mut ipset: IpSet, sets: Vec<Set>, shutdown: ShutdownToken) {
    shutdown.wait().await;
    for set in sets {
        set.destroy(&mut ipset).await;
    }
}
