use std::collections::BTreeSet;

use reaction_plugin::{
    ActionImpl, Hello, Manifest, PluginInfo, RemoteError, RemoteResult, StreamImpl, Value,
    shutdown::ShutdownController,
};
use remoc::rtc;

use crate::{
    action::{Action, ActionOptions},
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

        let options: ActionOptions = serde_json::from_value(config.into()).map_err(|err| {
            format!("invalid options for action {stream_name}.{filter_name}.{action_name}: {err}")
        })?;

        let ip_index = patterns
            .into_iter()
            .enumerate()
            .filter(|(_, name)| name == &options.pattern)
            .next()
            .ok_or_else(|| {
                format!(
                    "No pattern with name {} in filter {stream_name}.{filter_name}. Try setting the option `pattern` to your pattern name of type 'ip'",
                    options.pattern
                )
            })?
            .0;

        let (tx, rx) = remoc::rch::mpsc::channel(1);
        self.actions.push(Action::new(
            self.ipset.clone(),
            self.shutdown.token(),
            ip_index,
            rx,
            options,
        )?);

        Ok(ActionImpl { tx })
    }

    async fn finish_setup(&mut self) -> RemoteResult<()> {
        // Init all sets
        let mut first_error = None;
        for (i, action) in self.actions.iter_mut().enumerate() {
            // Retain if error
            if let Err(err) = action.init().await {
                first_error = Some((i, RemoteError::Plugin(err)));
                break;
            }
        }
        // Destroy initialized sets if error
        if let Some((i, err)) = first_error {
            for action in self.actions.iter_mut().take(i + 1) {
                let _ = action.destroy().await;
            }
            return Err(err);
        }

        // Launch all actions
        while let Some(action) = self.actions.pop() {
            tokio::spawn(async move { action.serve() });
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
