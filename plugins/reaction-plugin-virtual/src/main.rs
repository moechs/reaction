use std::collections::{BTreeMap, BTreeSet};

use reaction_plugin::{
    ActionConfig, ActionImpl, Exec, Hello, Line, Manifest, PluginInfo, RemoteResult, StreamConfig,
    StreamImpl, Value, line::PatternLine,
};
use remoc::{rch::mpsc, rtc};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() {
    let plugin = Plugin::default();
    reaction_plugin::main_loop(plugin).await;
}

#[derive(Default)]
struct Plugin {}

impl PluginInfo for Plugin {
    async fn manifest(&mut self) -> Result<Manifest, rtc::CallError> {
        Ok(Manifest {
            hello: Hello::new(),
            streams: BTreeSet::from(["virtual".into()]),
            actions: BTreeSet::from(["virtual".into()]),
        })
    }

    async fn load_config(
        &mut self,
        streams: Vec<StreamConfig>,
        actions: Vec<ActionConfig>,
    ) -> RemoteResult<(Vec<StreamImpl>, Vec<ActionImpl>)> {
        let mut ret_streams = Vec::with_capacity(streams.len());
        let mut ret_actions = Vec::with_capacity(actions.len());

        let mut local_streams = BTreeMap::new();

        for StreamConfig {
            stream_name,
            stream_type,
            config,
        } in streams
        {
            if stream_type != "virtual" {
                return Err("This plugin can't handle other stream types than virtual".into());
            }

            let (virtual_stream, receiver) = VirtualStream::new(config)?;

            if let Some(_) = local_streams.insert(stream_name, virtual_stream) {
                return Err("this virtual stream has already been initialized".into());
            }

            ret_streams.push(StreamImpl {
                stream: receiver,
                standalone: false,
            });
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
            if &action_type != "virtual" {
                return Err("This plugin can't handle other action types than virtual".into());
            }

            let (mut virtual_action, tx) = VirtualAction::new(
                stream_name,
                filter_name,
                action_name,
                config,
                patterns,
                &local_streams,
            )?;

            tokio::spawn(async move { virtual_action.serve().await });

            ret_actions.push(ActionImpl { tx });
        }

        Ok((ret_streams, ret_actions))
    }

    async fn start(&mut self) -> RemoteResult<()> {
        Ok(())
    }

    async fn close(self) -> RemoteResult<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct VirtualStream {
    tx: mpsc::Sender<Line>,
}

impl VirtualStream {
    fn new(config: Value) -> Result<(Self, mpsc::Receiver<Line>), String> {
        const CONFIG_ERROR: &'static str = "streams of type virtual take no options";
        match config {
            Value::Null => (),
            Value::Object(map) => {
                if map.len() != 0 {
                    return Err(CONFIG_ERROR.into());
                }
            }
            _ => return Err(CONFIG_ERROR.into()),
        }

        let (tx, rx) = mpsc::channel(1);
        Ok((Self { tx }, rx))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionOptions {
    /// The line to send to the corresponding virtual stream, example: "ban \<ip\>"
    send: String,
    /// The name of the corresponding virtual stream, example: "my_stream"
    to: String,
}

struct VirtualAction {
    rx: mpsc::Receiver<Exec>,
    send: PatternLine,
    to: VirtualStream,
}

impl VirtualAction {
    fn new(
        stream_name: String,
        filter_name: String,
        action_name: String,
        config: Value,
        patterns: Vec<String>,
        streams: &BTreeMap<String, VirtualStream>,
    ) -> Result<(Self, mpsc::Sender<Exec>), String> {
        let options: ActionOptions = serde_json::from_value(config.into()).map_err(|err| {
            format!("invalid options for action {stream_name}.{filter_name}.{action_name}: {err}")
        })?;

        let send = PatternLine::new(options.send, patterns);

        let stream = streams.get(&options.to).ok_or_else(|| {
            format!(
                "action {}.{}.{}: send \"{}\" matches no stream name",
                stream_name, filter_name, action_name, options.to
            )
        })?;

        let (tx, rx) = mpsc::channel(1);
        Ok((
            Self {
                rx,
                send: send,
                to: stream.clone(),
            },
            tx,
        ))
    }

    async fn serve(&mut self) {
        while let Ok(Some(exec)) = self.rx.recv().await {
            let line = self.send.line(exec.match_);
            self.to.tx.send((line, exec.time)).await.unwrap();
        }
    }
}
