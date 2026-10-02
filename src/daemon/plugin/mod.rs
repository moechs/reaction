use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    io,
    ops::{Deref, DerefMut},
    process::ExitStatus,
    time::Duration,
};

use futures::{StreamExt, future::join_all};
use reaction_plugin::{
    ActionConfig, ActionImpl, Hello, PluginInfo, PluginInfoClient, StreamConfig, StreamImpl,
};
use remoc::Connect;
use tokio::{
    process::{Child, ChildStderr},
    time::timeout,
};
use tracing::{error, info};

use crate::{
    concepts::{Action, Config, Plugin, Stream},
    daemon::{ShutdownToken, stream::reader_to_stream, utils::kill_child},
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
            .map_err(|err| systemd_error(plugin, "could not launch plugin", err))?;

        {
            let stderr = child.stderr.take().unwrap();
            // let shutdown = shutdown.clone();
            tokio::spawn(async move { handle_stderr(stderr, plugin.name.clone()).await });
        }

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let (conn, _tx, mut rx): (
            _,
            remoc::rch::base::Sender<()>,
            remoc::rch::base::Receiver<PluginInfoClient>,
        ) = Connect::io(remoc::Cfg::default(), stdout, stdin)
            .await
            .map_err(|err| {
                systemd_error(plugin, "could not init communication with plugin", err)
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

        let my_hello = Hello::new();

        if let Err(hint) = Hello::is_compatible(&my_hello, &manifest.hello) {
            return Err(format!(
                "reaction can't handle plugin {} with incompatible version {}.{}: current version: {}.{}. {}",
                plugin.name,
                manifest.hello.version_major,
                manifest.hello.version_minor,
                my_hello.version_major,
                my_hello.version_minor,
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
        tokio::select! {
            status = self.child.wait() => {
                self.print_exit(status);
                return;
            }
            _ = self.shutdown.wait() => {}
        }

        match timeout(
            Duration::from_secs(PLUGIN_STOP_GRACE_TIME),
            self.plugin_info.close(),
        )
        .await
        {
            Ok(Ok(())) => (),
            Ok(Err(err)) => {
                error!("plugin {}: {err}", self.plugin.name);
            }
            // got timeout
            Err(_) => {
                error!(
                    "plugin {} did not respond to close request in time, killing",
                    self.plugin.name
                );
                kill_child(self.child, format!("plugin {}", self.plugin.name), 5).await;
            }
        }
    }

    fn print_exit(&self, status: io::Result<ExitStatus>) {
        match status {
            Ok(status) => match status.code() {
                Some(code) => {
                    error!(
                        "plugin {}: process exited. exit code: {}",
                        self.plugin.name, code
                    );
                }
                None => {
                    error!("plugin {}: process exited.", self.plugin.name);
                }
            },
            Err(err) => {
                error!("plugin {}: process exited. {err}", self.plugin.name);
            }
        }
    }
}

fn systemd_error(plugin: &Plugin, message: &str, err: impl Display) -> String {
    if plugin.systemd {
        format!(
            "{message}: {err}. \
`plugins.{0}.systemd` is set to true, so this may be an issue with systemd's run0. \
please make sure `sudo run0 ls /` returns the same thing as `sudo ls /` as a test. \
if run0 can't be found or doesn't output anything, set `plugins.{0}.systemd` to false.",
            plugin.name,
        )
    } else {
        format!("{message}: {err}")
    }
}

async fn handle_stderr(stderr: ChildStderr, plugin_name: String) {
    // read lines until shutdown
    let lines = reader_to_stream(stderr);
    tokio::pin!(lines);
    loop {
        match lines.next().await {
            Some(Ok(line)) => {
                // sad: I can't factorize this because the tracing::event! macro
                // requires its log level to be a constant.
                if line.starts_with("DEBUG ") {
                    tracing::debug!("plugin {plugin_name}: {}", line.split_at(6).1)
                } else if line.starts_with("INFO ") {
                    tracing::info!("plugin {plugin_name}: {}", line.split_at(5).1)
                } else if line.starts_with("WARN ") {
                    tracing::warn!("plugin {plugin_name}: {}", line.split_at(5).1)
                } else if line.starts_with("ERROR ") {
                    tracing::error!("plugin {plugin_name}: {}", line.split_at(6).1)
                } else {
                    // If there is no log level, we suppose it's an error (may be a panic or something)
                    tracing::error!("plugin {plugin_name}: {}", line)
                }
            }
            Some(Err(err)) => {
                tracing::error!("while trying to read plugin {plugin_name} stderr: {err}");
                break;
            }
            None => break,
        }
    }
}

#[derive(Default)]
pub struct Plugins {
    /// Loaded plugins
    plugins: BTreeMap<String, PluginManager>,
    /// stream_type to plugin name
    stream_to_plugin: BTreeMap<String, String>,
    /// action_type to plugin name
    action_to_plugin: BTreeMap<String, String>,
    /// plugin name to config list
    plugin_to_confs: BTreeMap<String, (Vec<&'static Stream>, Vec<&'static Action>)>,
    /// stream name to impl
    stream_to_impl: BTreeMap<String, StreamImpl>,
    /// action name to impl
    action_to_impl: BTreeMap<String, ActionImpl>,
}

impl Plugins {
    pub async fn new(config: &'static Config, shutdown: ShutdownToken) -> Result<Self, String> {
        let mut this = Self::default();

        for plugin in config.plugins.values() {
            let name = plugin.name.clone();
            this.load_plugin(plugin, &config.state_directory, shutdown.clone())
                .await
                .map_err(|err| format!("plugin {name}: {err}]"))?;
        }

        this.aggregate_plugin_configs(config)?;

        this.load_plugin_configs().await?;

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
            if let Some(name) = self.stream_to_plugin.insert(stream.clone(), name.clone()) {
                return Err(format!(
                    "plugin {name} already exposed a stream with type name '{stream}'",
                ));
            }
        }

        for action in &manager.actions {
            if let Some(name) = self.action_to_plugin.insert(action.clone(), name.clone()) {
                return Err(format!(
                    "plugin {name} already exposed a action with type name '{action}'",
                ));
            }
        }

        self.plugins.insert(name, manager);
        Ok(())
    }

    fn aggregate_plugin_configs(&mut self, config: &'static Config) -> Result<(), String> {
        for stream in config.streams.values() {
            if stream.is_plugin()
                && let Some(stream_type) = &stream.stream_type
            {
                let plugin_name = self.stream_to_plugin.get(stream_type).ok_or_else(|| {
                    display_plugin_exposed_types(&self.stream_to_plugin, "stream", stream_type)
                })?;
                let (streams, _) = self
                    .plugin_to_confs
                    .entry(plugin_name.to_owned())
                    .or_default();
                streams.push(stream);
            }

            for action in stream
                .filters
                .values()
                .flat_map(|filter| filter.actions.values())
            {
                if action.is_plugin()
                    && let Some(action_type) = &action.action_type
                {
                    let plugin_name = self.action_to_plugin.get(action_type).ok_or_else(|| {
                        display_plugin_exposed_types(&self.action_to_plugin, "action", action_type)
                    })?;
                    let (_, actions) = self
                        .plugin_to_confs
                        .entry(plugin_name.to_owned())
                        .or_default();
                    actions.push(action);
                }
            }
        }
        Ok(())
    }

    async fn load_plugin_configs(&mut self) -> Result<(), String> {
        let plugin_to_confs = std::mem::take(&mut self.plugin_to_confs);
        for (plugin_name, (streams, actions)) in plugin_to_confs {
            let plugin = self
                .plugins
                .get_mut(&plugin_name)
                .ok_or_else(|| format!("could not find plugin {plugin_name}. this is a bug!"))?;

            let stream_names: Vec<String> =
                streams.iter().map(|stream| stream.name.clone()).collect();
            let action_names: Vec<String> =
                actions.iter().map(|action| action.to_string()).collect();

            let top_options = plugin
                .plugin
                .options()
                .map(|o| o.to_owned().into())
                .collect();

            let (stream_impls, action_impls) = plugin
                .load_config(
                    streams
                        .into_iter()
                        .map(Stream::to_stream_config)
                        .collect::<Result<Vec<StreamConfig>, String>>()?,
                    actions
                        .into_iter()
                        .map(Action::to_action_config)
                        .collect::<Result<Vec<ActionConfig>, String>>()?,
                    top_options,
                )
                .await
                .map_err(|err| {
                    format!("plugin {plugin_name} is not happy with your config: {err}")
                })?;

            self.stream_to_impl
                .extend(stream_names.into_iter().zip(stream_impls));
            self.action_to_impl
                .extend(action_names.into_iter().zip(action_impls));
        }

        Ok(())
    }

    pub fn get_stream_impl(&mut self, stream_name: String) -> Option<StreamImpl> {
        self.stream_to_impl.remove(&stream_name)
    }

    pub fn get_action_impl(&mut self, action_fullname: String) -> Option<ActionImpl> {
        self.action_to_impl.remove(&action_fullname)
    }

    pub async fn start(&mut self) -> Result<(), String> {
        // Finish setup of all plugins
        join_all(
            self.plugins
                .values_mut()
                .map(|plugin_manager| plugin_manager.start()),
        )
        .await
        // Convert Vec<Result<Result>> into Result
        .into_iter()
        .zip(self.plugins.values())
        .try_for_each(|(result, plugin_manager)| {
            result.map_err(|err| {
                format!(
                    "plugin {}: {}",
                    plugin_manager.plugin.name,
                    err.to_string().replace('\n', " ")
                )
            })
        })
    }

    pub fn manager(self) {
        for plugin in self.plugins.into_values() {
            tokio::spawn(async move {
                plugin.handle_child().await;
            });
        }
    }
}

fn display_plugin_exposed_types(
    type_to_plugin: &BTreeMap<String, String>,
    name: &str,
    invalid: &str,
) -> String {
    let mut plugin_to_types: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (type_, plugin) in type_to_plugin {
        plugin_to_types.entry(plugin).or_default().push(type_);
    }
    for (plugin, types) in plugin_to_types {
        info!(
            "Plugin {plugin} exposes those {name} types: '{}'",
            types.join("', '")
        );
    }
    format!("No plugin provides the {name} type: {invalid}")
}
