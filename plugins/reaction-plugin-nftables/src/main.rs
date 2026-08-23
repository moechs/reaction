use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use nftables::{
    batch::Batch,
    schema::{Chain, NfListObject, Table},
    types::{NfChainType, NfFamily},
};
use reaction_plugin::{
    ActionConfig, ActionImpl, Hello, Manifest, PluginInfo, RemoteResult, StreamConfig, StreamImpl,
    Value, shutdown::ShutdownController,
};
use remoc::rtc;

use crate::{
    action::{Action, ActionOptions, Set, SetOptions},
    nft::NftClient,
};

#[cfg(test)]
mod tests;

mod action;
pub mod helpers;
mod nft;

#[tokio::main]
async fn main() {
    let plugin = Plugin::default();
    reaction_plugin::main_loop(plugin).await;
}

#[derive(Default)]
struct Plugin {
    nft: NftClient,
    sets: Vec<Set>,
    actions: Vec<Action>,
    shutdown: ShutdownController,
}

impl PluginInfo for Plugin {
    async fn manifest(&mut self) -> Result<Manifest, rtc::CallError> {
        Ok(Manifest {
            hello: Hello::new(),
            streams: BTreeSet::default(),
            actions: BTreeSet::from(["nftables".into()]),
        })
    }

    async fn load_config(
        &mut self,
        streams: Vec<StreamConfig>,
        actions: Vec<ActionConfig>,
        options: Vec<Value>,
    ) -> RemoteResult<(Vec<StreamImpl>, Vec<ActionImpl>)> {
        if !options.is_empty() {
            return Err("No top-level options are allowed for this plugin at the time".into());
        }

        if !streams.is_empty() {
            return Err("This plugin can't handle any stream type".into());
        }

        let mut ret_actions = Vec::with_capacity(actions.len());
        let mut set_options: BTreeMap<String, SetOptions> = BTreeMap::new();

        for ActionConfig {
            stream_name,
            filter_name,
            action_name,
            action_type,
            config,
            patterns,
        } in actions
        {
            if &action_type != "nftables" {
                return Err("This plugin can't handle other action types than nftables".into());
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
            set_options
                .entry(options.set.clone())
                .or_default()
                .merge(&options.set_options)
                .map_err(|err| format!("set {}: {err}", options.set))?;

            let (tx, rx) = remoc::rch::mpsc::channel(1);
            self.actions.push(Action::new(
                self.nft.clone(),
                self.shutdown.token(),
                rx,
                options,
            )?);

            ret_actions.push(ActionImpl { tx });
        }

        // Init all sets
        while let Some((name, options)) = set_options.pop_first() {
            self.sets.push(Set::from(name, options));
        }

        Ok((vec![], ret_actions))
    }

    async fn start(&mut self) -> RemoteResult<()> {
        self.shutdown.delegate().handle_quit_signals()?;

        let mut batch = Batch::new();
        batch.add(reaction_table());

        // Create a chain for each registered netfilter hook
        for hook in self
            .sets
            .iter()
            .flat_map(|set| &set.hooks)
            .collect::<BTreeSet<_>>()
        {
            batch.add(NfListObject::Chain(Chain {
                family: NfFamily::INet,
                table: Cow::Borrowed("reaction"),
                name: Cow::from(hook.as_str()),
                _type: Some(NfChainType::Filter),
                hook: Some(hook.into()),
                prio: Some(0),
                ..Default::default()
            }));
        }

        for set in &self.sets {
            set.init(&mut batch)?;
        }

        // apply batch
        self.nft.send(batch).await?;

        // Launch a task that will destroy the table on shutdown
        {
            let token = self.shutdown.token();
            let nft = self.nft.clone();
            tokio::spawn(async move {
                token.wait().await;

                let mut batch = Batch::new();
                batch.delete(reaction_table());
                if let Err(err) = nft.send(batch).await {
                    eprintln!("ERROR removing reaction nft table: {err}");
                }
            });
        }

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

fn reaction_table() -> NfListObject<'static> {
    NfListObject::Table(Table {
        family: NfFamily::INet,
        name: Cow::Borrowed("reaction"),
        handle: None,
    })
}
