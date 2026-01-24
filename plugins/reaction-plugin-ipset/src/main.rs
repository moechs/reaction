use std::collections::BTreeSet;

use reaction_plugin::{
    ActionImpl, Exec, Hello, Manifest, PluginInfo, RemoteResult, StreamImpl, Value,
};
use remoc::{rch::mpsc, rtc};
use serde::{Deserialize, Serialize, de::Deserializer, de::Error};

use crate::ipset::ipset_thread;

#[cfg(test)]
mod tests;

mod ipset;

#[tokio::main]
async fn main() {
    let plugin = Plugin::default();
    reaction_plugin::main_loop(plugin).await;
}

#[derive(Default)]
struct Plugin {
    // ipset: Arc<Mutex<IPSet>>,
    actions: Vec<Action>,
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

        let ip_index = patterns
            .into_iter()
            .enumerate()
            .filter(|(_, name)| name == &options.pattern)
            .next()
            .ok_or_else(|| {
                format!(
                    "No pattern with name {} in filter {stream_name}.{filter_name}",
                    options.pattern
                )
            })?
            .0;

        let (tx, rx) = remoc::rch::mpsc::channel(1);
        self.actions.push(Action {
            chains: options.chains,
            ipv4_set: match options.version {
                IpVersion::V4 => Some(options.set.clone()),
                IpVersion::V6 => None,
                IpVersion::V46 => Some(format!("{}v4", options.set)),
            },
            ipv6_set: match options.version {
                IpVersion::V4 => None,
                IpVersion::V6 => Some(options.set),
                IpVersion::V46 => Some(format!("{}v6", options.set)),
            },
            ip_index,
            rx,
        });

        Ok(ActionImpl { tx })
    }

    async fn finish_setup(&mut self) -> RemoteResult<()> {
        ipset_thread()?;

        todo!();
    }

    async fn close(self) -> RemoteResult<()> {
        todo!();
    }
}

enum IpVersion {
    V4,
    V6,
    V46,
}
impl<'de> Deserialize<'de> for IpVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<u8>::deserialize(deserializer)? {
            None => Ok(IpVersion::V46),
            Some(version) => match version {
                4 => Ok(IpVersion::V4),
                6 => Ok(IpVersion::V6),
                46 => Ok(IpVersion::V46),
                _ => Err(D::Error::custom("version must be one of 4, 6 or 46")),
            },
        }
    }
}
impl Serialize for IpVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(match self {
            IpVersion::V4 => 4,
            IpVersion::V6 => 6,
            IpVersion::V46 => 46,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionOptions {
    /// The set that should be used by this action
    set: String,
    /// The pattern name of the IP.
    /// Defaults to "ip"
    #[serde(default = "serde_ip")]
    pattern: String,
    /// The IP type.
    /// Defaults to `46`.
    /// If `4`: creates an IPv4 set with this name
    /// If `6`: creates an IPv6 set with this name
    /// If `46`: creates an IPv4 set with its name suffixed by 'v4' AND an IPv6 set with its name suffixed by 'v6'
    version: IpVersion,
    /// Chains where the IP set should be inserted.
    /// Defaults to `["INPUT", "FORWARD"]`
    #[serde(default = "serde_chains")]
    chains: Vec<String>,
    // Optional timeout, letting linux/netfilter handle set removal instead of reaction
    // Note that `reaction show` and `reaction flush` won't work if set instead of an `after` action
}

fn serde_ip() -> String {
    "ip".into()
}
fn serde_chains() -> Vec<String> {
    vec!["INPUT".into(), "FORWARD".into()]
}

struct Action {
    ipv4_set: Option<String>,
    ipv6_set: Option<String>,
    // index of pattern ip in match vec
    ip_index: usize,
    chains: Vec<String>,
    rx: mpsc::Receiver<Exec>,
}

impl Action {
    async fn serve(&mut self) {
        // while let Ok(Some(exec)) = self.rx.recv().await {
        //     let line = self.send.line(exec.match_);
        //     self.to.tx.send((line, exec.time)).await.unwrap();
        // }
    }
}
