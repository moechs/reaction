use std::collections::{BTreeMap, BTreeSet};

use reaction_plugin::{
    ActionImpl, Exec, Hello, Line, Local, Manifest, PluginInfo, RemoteResult, StreamImpl, Value,
};
use remoc::{rch::mpsc, rtc};
use serde::{Deserialize, Serialize};

#[tokio::main]
async fn main() {
    let plugin = Plugin::default();
    reaction_plugin::main_loop(plugin).await;
}

#[derive(Default)]
struct Plugin {
    streams: BTreeMap<String, VirtualStream>,
    actions_init: Vec<VirtualActionInit>,
}

impl PluginInfo for Plugin {
    async fn manifest(&mut self) -> Result<Manifest, rtc::CallError> {
        Ok(Manifest {
            hello: Hello::new(),
            streams: BTreeSet::from(["virtual".into()]),
            actions: BTreeSet::from(["virtual".into()]),
        })
    }

    async fn stream_impl(
        &mut self,
        stream_name: String,
        stream_type: String,
        config: Value,
    ) -> RemoteResult<StreamImpl> {
        if stream_type != "virtual" {
            return Err("This plugin can't handle other stream types than virtual".into());
        }

        let (virtual_stream, receiver) = VirtualStream::new(config)?;

        if let Some(_) = self.streams.insert(stream_name, virtual_stream) {
            return Err("this virtual stream has already been initialized".into());
        }

        Ok(StreamImpl {
            stream: receiver,
            standalone: false,
        })
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
        if &action_type != "virtual" {
            return Err("This plugin can't handle other action types than virtual".into());
        }

        let (virtual_action_init, tx) =
            VirtualActionInit::new(stream_name, filter_name, action_name, config, patterns)?;

        self.actions_init.push(virtual_action_init);
        Ok(ActionImpl { tx })
    }

    async fn finish_setup(&mut self) -> RemoteResult<()> {
        while let Some(action_init) = self.actions_init.pop() {
            match self.streams.get(&action_init.to) {
                Some(virtual_stream) => {
                    let virtual_stream = virtual_stream.clone();
                    tokio::spawn(async move {
                        VirtualAction::from(action_init, virtual_stream)
                            .serve()
                            .await
                    });
                }
                None => {
                    return Err(format!(
                        "action {}.{}.{}: send \"{}\" matches no stream name",
                        action_init.stream_name,
                        action_init.filter_name,
                        action_init.action_name,
                        action_init.to
                    )
                    .into());
                }
            }
        }
        // Free containers
        self.streams = Default::default();
        self.actions_init = Default::default();

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
struct ActionOptions {
    /// The line to send to the corresponding virtual stream, example: "ban \<ip\>"
    send: String,
    /// The name of the corresponding virtual stream, example: "my_stream"
    to: String,
}

struct VirtualActionInit {
    stream_name: String,
    filter_name: String,
    action_name: String,
    rx: mpsc::Receiver<Exec>,
    patterns: Vec<String>,
    send: String,
    to: String,
}

impl VirtualActionInit {
    fn new(
        stream_name: String,
        filter_name: String,
        action_name: String,
        config: Value,
        patterns: Vec<String>,
    ) -> Result<(Self, mpsc::Sender<Exec>), String> {
        let options: ActionOptions = serde_json::from_value(config.into()).map_err(|err| {
            format!("invalid options for action {stream_name}.{filter_name}.{action_name}: {err}")
        })?;

        let patterns = patterns
            .into_iter()
            .map(|pattern| format!("<{pattern}>"))
            .collect();

        let (tx, rx) = mpsc::channel(1);
        Ok((
            Self {
                stream_name,
                filter_name,
                action_name,
                rx,
                patterns,
                send: options.send,
                to: options.to,
            },
            tx,
        ))
    }
}

struct VirtualAction {
    rx: mpsc::Receiver<Exec>,
    patterns: Vec<String>,
    send: String,
    to: VirtualStream,
}

impl VirtualAction {
    fn from(action_init: VirtualActionInit, to: VirtualStream) -> VirtualAction {
        VirtualAction {
            rx: action_init.rx,
            patterns: action_init.patterns,
            send: action_init.send,
            to,
        }
    }

    async fn serve(&mut self) {
        while let Ok(Some(m)) = self.rx.recv().await {
            let line = if m.match_.is_empty() {
                self.send.clone()
            } else {
                (0..(m.match_.len()))
                    .zip(&self.patterns)
                    .fold(self.send.clone(), |acc, (i, pattern)| {
                        acc.replace(pattern, &m.match_[i])
                    })
            };
            let result = match self.to.tx.send((line, Local::now())).await {
                Ok(_) => Ok(()),
                Err(err) => Err(format!("{err}")),
            };
            m.result.send(result).unwrap();
        }
    }
}
