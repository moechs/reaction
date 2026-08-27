use std::{collections::BTreeSet, time::Duration};

use async_trait::async_trait;
use pingora_core::server::{
    RunArgs, Server, ShutdownSignal, ShutdownSignalWatch, UnixShutdownSignalWatch,
    configuration::ServerConf,
};
use pingora_proxy::http_proxy_service;
use reaction_plugin::{
    ActionConfig, ActionImpl, Hello, Manifest, PluginInfo, RemoteResult, StreamConfig, StreamImpl,
    Value,
};
use remoc::rtc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::{
    action::ip_sets_from_names,
    config::{ActionOptions, MainOptions},
    proxy::Proxy,
};

mod action;
mod config;
mod proxy;

#[tokio::main]
async fn main() {
    let plugin = MyPlugin::default();
    reaction_plugin::main_loop(plugin).await;
}

#[derive(Default)]
struct MyPlugin {
    server: Option<Server>,
    token: CancellationToken,
}

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

        let ip_sets = ip_sets_from_names(
            computed_main_options
                .domains
                .values()
                .flat_map(|domain| domain.cases.keys().cloned()),
        );

        let mut computed_action_options = Vec::new();
        let mut action_sets = BTreeSet::new();
        let mut ret_actions = Vec::new();

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

            if let Some(set) = &options.add {
                action_sets.insert(set.clone());
            }
            if let Some(set) = &options.remove {
                action_sets.insert(set.clone());
            }

            let (tx, rx) = remoc::rch::mpsc::channel(1);

            match options.setup(&patterns, &ip_sets, rx) {
                Ok(computed) => {
                    computed_action_options.push(computed);
                    ret_actions.push(ActionImpl { tx });
                }
                Err(err) => match err {
                    Some(err) => Err(format!("invalid action {name}: {err}"))?,
                    None => {
                        // That's fine, we will return an error later about the set mismatch between domains and actions
                    }
                },
            }
        }

        // check that all sets referenced by domains or by actions are used by the each others.
        {
            let domain_sets: BTreeSet<String> = computed_main_options
                .domains
                .values()
                .flat_map(|domain| domain.cases.keys().cloned())
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

        for action in computed_action_options {
            tokio::spawn(async move { action.serve().await });
        }

        let proxy = Proxy::new(computed_main_options);
        let mut server = Server::new_with_opt_and_conf(
            None,
            ServerConf {
                daemon: false,
                grace_period_seconds: Some(2),
                graceful_shutdown_timeout_seconds: Some(2),
                ..Default::default()
            },
        );
        server.bootstrap();
        let my_proxy = http_proxy_service(&server.configuration, proxy);
        server.add_service(my_proxy);
        self.server = Some(server);

        Ok((vec![], ret_actions))
    }

    async fn start(&mut self) -> RemoteResult<()> {
        let (token, shutdown) = Shutdown::new();
        self.token = token;

        self.server
            .take()
            .ok_or_else(|| format!("plugin has not been initialized. this is a bug!"))?
            .run(RunArgs {
                shutdown_signal: Box::new(shutdown),
            });
        Ok(())
    }

    async fn close(self) -> RemoteResult<()> {
        self.token.cancel();
        // FIXME hook on the cancellation instead of sleeping 4 seconds
        sleep(Duration::from_secs(4)).await;
        Ok(())
    }
}

pub struct Shutdown {
    token: CancellationToken,
    unix: UnixShutdownSignalWatch,
}
impl Shutdown {
    pub fn new() -> (CancellationToken, Self) {
        let token = CancellationToken::new();
        let unix = UnixShutdownSignalWatch;
        (token.clone(), Self { token, unix })
    }
}
#[async_trait]
impl ShutdownSignalWatch for Shutdown {
    async fn recv(&self) -> ShutdownSignal {
        let cancel = self.token.clone().cancelled_owned();
        tokio::select! {
            _ = cancel => ShutdownSignal::GracefulTerminate,
            ret = self.unix.recv() => ret,
        }
    }
}
