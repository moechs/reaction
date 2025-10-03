use std::collections::BTreeMap;

use reaction_plugin::{ActionImpl, Exec, Line, PluginInfo, RemoteResult, StreamImpl, Value};
use remoc::rch::mpsc;

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
    async fn stream_impls(&self) -> RemoteResult<Vec<String>> {
        Ok(vec!["virtual".into()])
    }

    async fn action_impls(&self) -> RemoteResult<Vec<String>> {
        Ok(vec!["virtual".into()])
    }

    async fn stream_impl(
        &mut self,
        stream_name: String,
        stream_type: String,
        config: Value,
    ) -> RemoteResult<Result<StreamImpl, String>> {
        if stream_type != "virtual" {
            return Ok(Err(
                "This plugin can't handle other stream types than virtual".into(),
            ));
        }

        let (virtual_stream, receiver) = match VirtualStream::new(config) {
            Ok(v) => v,
            Err(err) => return Ok(Err(err)),
        };

        if let Some(_) = self.streams.insert(stream_name, virtual_stream) {
            return Ok(Err(
                "this virtual stream has already been initialized".into()
            ));
        }

        Ok(Ok(StreamImpl { stream: receiver }))
    }

    async fn action_impl(
        &mut self,
        stream_name: String,
        filter_name: String,
        action_name: String,
        action_type: String,
        config: Value,
        patterns: Vec<String>,
    ) -> RemoteResult<Result<ActionImpl, String>> {
        if &action_type != "virtual" {
            return Ok(Err(
                "This plugin can't handle other stream types than virtual".into(),
            ));
        }

        let (virtual_action_init, tx) =
            match VirtualActionInit::new(stream_name, filter_name, action_name, config, patterns) {
                Ok(v) => v,
                Err(err) => return Ok(Err(err)),
            };

        self.actions_init.push(virtual_action_init);
        Ok(Ok(ActionImpl { tx }))
    }

    async fn finish_setup(&mut self) -> RemoteResult<Result<(), String>> {
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
                    return Ok(Err(format!(
                        "action {}.{}.{}: send \"{}\" matches no stream name",
                        action_init.stream_name,
                        action_init.filter_name,
                        action_init.action_name,
                        action_init.to
                    )));
                }
            }
        }
        self.streams = BTreeMap::new();
        self.actions_init = Vec::new();

        Ok(Ok(()))
    }

    async fn close(self) -> RemoteResult<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct VirtualStream {
    tx: mpsc::Sender<Result<String, String>>,
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

        let (tx, rx) = mpsc::channel(0);
        Ok((Self { tx }, rx))
    }
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
        let send;
        let to;
        match config {
            Value::Object(mut map) => {
                send = match map.remove("send") {
                    Some(Value::String(value)) => value,
                    _ =>  return Err("`send` must be a string to send to the corresponding virtual stream, example: \"ban <ip>\"".into()),
                };

                to = match map.remove("to") {
                    Some(Value::String(value)) => value,
                    _ =>  return Err("`to` must be the name of the corresponding virtual stream, example: \"my_stream\"".into()),
                };

                if map.len() != 0 {
                    return Err(
                        "actions of type virtual accept only `send` and `to` options".into(),
                    );
                }
            }
            _ => {
                return Err("actions of type virtual require `send` and `to` options".into());
            }
        }

        let patterns = patterns
            .into_iter()
            .map(|pattern| format!("<{pattern}>"))
            .collect();

        let (tx, rx) = mpsc::channel(0);
        Ok((
            Self {
                stream_name,
                filter_name,
                action_name,
                rx,
                patterns,
                send,
                to,
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
        loop {
            match self.rx.recv().await {
                Ok(Some(m)) => {
                    let line = if m.match_.is_empty() {
                        self.send.clone()
                    } else {
                        (0..(m.match_.len()))
                            .zip(&self.patterns)
                            .fold(self.send.clone(), |acc, (i, pattern)| {
                                acc.replace(pattern, &m.match_[i])
                            })
                    };
                    let result = match self.to.tx.send(Line::Ok(line)).await {
                        Ok(_) => Ok(()),
                        Err(err) => Err(format!("{err}")),
                    };
                    m.result.send(result).unwrap();
                }
                Ok(None) => {
                    return;
                }
                Err(_) => panic!(),
            }
        }
    }
}
