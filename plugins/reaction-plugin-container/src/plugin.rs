//! [`PluginInfo`] implementation: configuration, startup and shutdown.

use std::{collections::BTreeSet, time::Duration};

use reaction_plugin::{
    ActionConfig, ActionImpl, Hello, Manifest, PluginInfo, RemoteResult, StreamConfig, StreamImpl,
    Value, shutdown::ShutdownController, time::deser_duration,
};
use remoc::{rch::mpsc, rtc};
use serde::Deserialize;

#[cfg(feature = "docker")]
use crate::backend::docker;
use crate::core::{config::StreamOptions, supervisor::Sink};

/// Lines buffered on the plugin side of each stream channel.
const CHANNEL_CAPACITY: usize = 64;

/// `plugins.<name>.options`
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginOptions {
    /// Delay before retrying a failed runtime connection or log stream.
    #[serde(
        default = "default_reconnect_delay",
        deserialize_with = "deser_duration"
    )]
    pub reconnect_delay: Duration,
    /// When false (default), reaction fails to start if a runtime used by a
    /// stream is unreachable. When true, the plugin retries in the background.
    #[serde(default)]
    pub wait_for_runtime: bool,
    #[cfg(feature = "docker")]
    #[serde(default)]
    pub docker: docker::DockerOptions,
}

fn default_reconnect_delay() -> Duration {
    Duration::from_secs(5)
}

impl Default for PluginOptions {
    fn default() -> Self {
        Self {
            reconnect_delay: default_reconnect_delay(),
            wait_for_runtime: false,
            #[cfg(feature = "docker")]
            docker: Default::default(),
        }
    }
}

impl PluginOptions {
    pub fn parse(options: Vec<Value>) -> Result<Self, String> {
        let merged = options
            .into_iter()
            .try_fold(Value::Null, |acc, value| acc.merge(value))?;
        let options: Self = match merged {
            Value::Null => Self::default(),
            merged => serde_json::from_value(merged.into())
                .map_err(|err| format!("invalid plugin options: {err}"))?,
        };
        if options.reconnect_delay.is_zero() {
            return Err("reconnect_delay must be greater than 0".into());
        }
        #[cfg(feature = "docker")]
        options.docker.validate()?;
        Ok(options)
    }
}

#[derive(Default)]
pub struct Plugin {
    options: PluginOptions,
    #[cfg(feature = "docker")]
    docker_sinks: Vec<Sink>,
    shutdown: ShutdownController,
}

impl PluginInfo for Plugin {
    async fn manifest(&mut self) -> Result<Manifest, rtc::CallError> {
        #[allow(unused_mut)]
        let mut streams = BTreeSet::new();
        #[cfg(feature = "docker")]
        streams.insert(docker::STREAM_TYPE.to_string());
        Ok(Manifest {
            hello: Hello::new(),
            streams,
            actions: BTreeSet::new(),
        })
    }

    async fn load_config(
        &mut self,
        streams: Vec<StreamConfig>,
        actions: Vec<ActionConfig>,
        options: Vec<Value>,
    ) -> RemoteResult<(Vec<StreamImpl>, Vec<ActionImpl>)> {
        self.options = PluginOptions::parse(options)?;

        if let Some(action) = actions.first() {
            return Err(format!(
                "action {}.{}.{}: this plugin provides no action type",
                action.stream_name, action.filter_name, action.action_name
            )
            .into());
        }

        let mut stream_impls = Vec::with_capacity(streams.len());
        for StreamConfig {
            stream_name,
            stream_type,
            config,
        } in streams
        {
            // One arm per backend: parse with the backend's template keys and
            // hand the sink to that backend's supervisor.
            match stream_type.as_str() {
                #[cfg(feature = "docker")]
                docker::STREAM_TYPE => {
                    let options = StreamOptions::parse(&stream_name, config, docker::EXTRA_KEYS)?;
                    let (sink, receiver) = new_sink(stream_name, options);
                    self.docker_sinks.push(sink);
                    stream_impls.push(StreamImpl {
                        stream: receiver,
                        standalone: true,
                    });
                }
                other => {
                    let _ = config;
                    return Err(format!(
                        "stream {stream_name}: this plugin can't handle stream type {other:?}"
                    )
                    .into());
                }
            }
        }

        Ok((stream_impls, vec![]))
    }

    async fn start(&mut self) -> RemoteResult<()> {
        #[cfg(feature = "docker")]
        self.start_docker().await?;
        Ok(())
    }

    async fn close(self) -> RemoteResult<()> {
        self.shutdown.ask_shutdown();
        self.shutdown.wait_all_task_shutdown().await;
        Ok(())
    }
}

fn new_sink(
    stream_name: String,
    options: StreamOptions,
) -> (Sink, mpsc::Receiver<reaction_plugin::Line>) {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    (
        Sink {
            stream_name,
            options,
            tx,
        },
        rx,
    )
}

#[cfg(feature = "docker")]
impl Plugin {
    async fn start_docker(&mut self) -> Result<(), String> {
        use std::{sync::Arc, time::SystemTime};

        use crate::core::supervisor::Supervisor;

        if self.docker_sinks.is_empty() {
            return Ok(());
        }
        let sinks = std::mem::take(&mut self.docker_sinks);
        let options = self.options.clone();
        let token = self.shutdown.token();

        if !options.wait_for_runtime {
            let runtime = docker::DockerRuntime::connect(&options.docker).await?;
            let started_at = SystemTime::now();
            let supervisor = Supervisor::new(
                Arc::new(runtime),
                sinks,
                options.reconnect_delay,
                started_at,
                token,
            );
            tokio::spawn(supervisor.run());
            return Ok(());
        }

        tokio::spawn(async move {
            let runtime = loop {
                match docker::DockerRuntime::connect(&options.docker).await {
                    Ok(runtime) => break runtime,
                    Err(err) => {
                        log_warn!("docker: {err}; retrying in {:?}", options.reconnect_delay)
                    }
                }
                tokio::select! {
                    _ = token.wait() => return,
                    _ = tokio::time::sleep(options.reconnect_delay) => {}
                }
            };
            log_info!("docker: connected");
            let started_at = SystemTime::now();
            Supervisor::new(
                Arc::new(runtime),
                sinks,
                options.reconnect_delay,
                started_at,
                token,
            )
            .run()
            .await
        });
        Ok(())
    }
}
