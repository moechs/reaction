//! [`PluginInfo`] implementation: configuration, startup and shutdown.

use std::collections::BTreeSet;

use reaction_plugin::{
    ActionConfig, ActionImpl, Hello, Line, Manifest, PluginInfo, RemoteResult, StreamConfig,
    StreamImpl, Value, shutdown::ShutdownController,
};
use remoc::{rch::mpsc, rtc};
use tokio::net::{TcpListener, UdpSocket};

use crate::{
    backend,
    config::{PluginOptions, Protocol, StreamOptions},
    dispatch::Dispatcher,
    template::Template,
};

pub const STREAM_TYPE: &str = "syslog";
const CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Clone)]
pub struct Sink {
    pub stream_name: String,
    pub options: StreamOptions,
    pub template: Template,
    pub tx: mpsc::Sender<Line>,
}

#[derive(Default)]
pub struct Plugin {
    options: PluginOptions,
    sinks: Vec<Sink>,
    shutdown: ShutdownController,
}

impl PluginInfo for Plugin {
    async fn manifest(&mut self) -> Result<Manifest, rtc::CallError> {
        Ok(Manifest {
            hello: Hello::new(),
            streams: BTreeSet::from([STREAM_TYPE.to_string()]),
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
            if stream_type != STREAM_TYPE {
                return Err(format!(
                    "stream {stream_name}: this plugin can't handle stream type {stream_type:?}"
                )
                .into());
            }
            let options = StreamOptions::parse(&stream_name, config)?;
            let template = Template::parse(&options.prefix)
                .map_err(|err| format!("stream {stream_name}: invalid prefix: {err}"))?;
            let (tx, stream) = mpsc::channel(CHANNEL_CAPACITY);
            self.sinks.push(Sink {
                stream_name,
                options,
                template,
                tx,
            });
            stream_impls.push(StreamImpl {
                stream,
                standalone: true,
            });
        }

        Ok((stream_impls, vec![]))
    }

    async fn start(&mut self) -> RemoteResult<()> {
        match self.options.protocol {
            Protocol::Udp => self.start_udp().await?,
            Protocol::Tcp => self.start_tcp().await?,
        }
        Ok(())
    }

    async fn close(self) -> RemoteResult<()> {
        self.shutdown.ask_shutdown();
        self.shutdown.wait_all_task_shutdown().await;
        Ok(())
    }
}

impl Plugin {
    async fn start_udp(&mut self) -> Result<(), String> {
        let socket = UdpSocket::bind(self.options.listen)
            .await
            .map_err(|err| format!("cannot bind {}: {err}", self.options.listen))?;
        let dispatcher = Dispatcher::new(self.options.clone(), self.sinks.clone());
        let token = self.shutdown.token();
        tokio::spawn(async move {
            backend::udp::serve(socket, dispatcher, token).await;
        });
        Ok(())
    }

    async fn start_tcp(&mut self) -> Result<(), String> {
        let listener = TcpListener::bind(self.options.listen)
            .await
            .map_err(|err| format!("cannot bind {}: {err}", self.options.listen))?;
        let dispatcher = Dispatcher::new(self.options.clone(), self.sinks.clone());
        let shutdown = self.shutdown.clone();
        let token = self.shutdown.token();
        tokio::spawn(async move {
            backend::tcp::serve(listener, dispatcher, shutdown, token).await;
        });
        Ok(())
    }
}
