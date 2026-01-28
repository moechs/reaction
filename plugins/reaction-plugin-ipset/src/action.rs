use std::u32;

use reaction_plugin::{Exec, shutdown::ShutdownToken, time::parse_duration};
use remoc::rch::mpsc as remocMpsc;
use serde::{Deserialize, Serialize, de::Deserializer, de::Error};

use crate::ipset::{IpSet, Order, SetChain, SetOptions, Version};
pub enum IpVersion {
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

// FIXME block configs that have different set options for the same name
// treat default values as none?

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionOptions {
    /// The set that should be used by this action
    set: String,
    /// The pattern name of the IP.
    /// Defaults to "ip"
    #[serde(default = "serde_ip")]
    pub pattern: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<String>,
    // Target that iptables should use when the IP is encountered.
    // Defaults to DROP, but can also be ACCEPT, RETURN or any user-defined chain
    #[serde(default = "serde_drop")]
    target: String,
    // TODO add `add`//`remove` option
}

fn serde_ip() -> String {
    "ip".into()
}
fn serde_drop() -> String {
    "DROP".into()
}
fn serde_chains() -> Vec<String> {
    vec!["INPUT".into(), "FORWARD".into()]
}

pub struct Action {
    ipset: IpSet,
    rx: remocMpsc::Receiver<Exec>,
    shutdown: ShutdownToken,
    ipv4_set: Option<String>,
    ipv6_set: Option<String>,
    // index of pattern ip in match vec
    ip_index: usize,
    chains: Vec<String>,
    timeout: Option<u32>,
    target: String,
}

impl Action {
    pub fn new(
        ipset: IpSet,
        shutdown: ShutdownToken,
        ip_index: usize,
        rx: remocMpsc::Receiver<Exec>,
        options: ActionOptions,
    ) -> Result<Self, String> {
        Ok(Action {
            ipset,
            rx,
            shutdown,
            ip_index,
            target: options.target,
            chains: options.chains,
            timeout: if let Some(timeout) = options.timeout {
                let duration = parse_duration(&timeout)
                    .map_err(|err| format!("failed to parse timeout: {}", err))?
                    .as_secs();
                if duration > u32::MAX as u64 {
                    return Err(format!(
                        "timeout is limited to {} seconds (approx {} days)",
                        u32::MAX,
                        49_000
                    ));
                }
                Some(duration as u32)
            } else {
                None
            },
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
        })
    }

    pub async fn init(&mut self) -> Result<(), String> {
        for (set, version) in [
            (&self.ipv4_set, Version::IPv4),
            (&self.ipv6_set, Version::IPv6),
        ] {
            if let Some(set) = set {
                println!("INFO creating {version} set {set}");
                // create set
                self.ipset
                    .order(Order::CreateSet(SetOptions {
                        name: set.clone(),
                        version,
                        timeout: self.timeout,
                    }))
                    .await?;
                // insert set in chains
                for chain in &self.chains {
                    println!("INFO inserting {version} set {set} in chain {chain}");
                    self.ipset
                        .order(Order::InsertSet(SetChain {
                            set: set.clone(),
                            chain: chain.clone(),
                            target: self.target.clone(),
                        }))
                        .await?;
                }
            }
        }
        Ok(())
    }

    pub async fn destroy(&mut self) {
        for (set, version) in [
            (&self.ipv4_set, Version::IPv4),
            (&self.ipv6_set, Version::IPv6),
        ] {
            if let Some(set) = set {
                for chain in &self.chains {
                    println!("INFO removing {version} set {set} from chain {chain}");
                    if let Err(err) = self
                        .ipset
                        .order(Order::RemoveSet(SetChain {
                            set: set.clone(),
                            chain: chain.clone(),
                            target: self.target.clone(),
                        }))
                        .await
                    {
                        println!(
                            "ERROR while removing {version} set {set} from chain {chain}: {err}"
                        );
                    }
                }
                println!("INFO destroying {version} set {set}");
                if let Err(err) = self.ipset.order(Order::DestroySet(set.clone())).await {
                    println!("ERROR while destroying {version} set {set}: {err}");
                }
            }
        }
    }

    pub async fn serve(mut self) {
        loop {
            let event = tokio::select! {
                exec = self.rx.recv() => Some(exec),
                _ = self.shutdown.wait() => None,
            };
            match event {
                // shutdown asked
                None => break,
                // channel closed
                Some(Ok(None)) => break,
                // error from channel
                Some(Err(err)) => {
                    println!("ERROR {err}");
                    break;
                }
                // ok
                Some(Ok(Some(exec))) => {
                    if let Err(err) = self.handle_exec(exec).await {
                        println!("ERROR {err}");
                        break;
                    }
                }
            }
        }
        self.shutdown.ask_shutdown();
        self.destroy().await;
    }

    async fn handle_exec(&mut self, mut exec: Exec) -> Result<(), String> {
        // safeguard against Vec::remove's panic
        if exec.match_.len() <= self.ip_index {
            return Err(format!(
                "match received from reaction is smaller than expected. looking for index {} but size is {}. this is a bug!",
                self.ip_index,
                exec.match_.len()
            ));
        }
        let ip = exec.match_.remove(self.ip_index);
        // select set
        let set = match (&self.ipv4_set, &self.ipv6_set) {
            (None, None) => return Err(format!("action is neither IPv4 nor IPv6, this is a bug!")),
            (None, Some(set)) => set,
            (Some(set), None) => set,
            (Some(set4), Some(set6)) => {
                if ip.contains(':') {
                    set6
                } else {
                    set4
                }
            }
        };
        // add ip to set
        self.ipset.order(Order::Insert(set.clone(), ip)).await?;
        Ok(())
    }
}
