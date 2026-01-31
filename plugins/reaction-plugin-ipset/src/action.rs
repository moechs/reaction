use std::{fmt::Debug, u32};

use reaction_plugin::{Exec, shutdown::ShutdownToken, time::parse_duration};
use remoc::rch::mpsc as remocMpsc;
use serde::{Deserialize, Serialize};

use crate::ipset::{CreateSet, IpSet, Order, SetChain, Version};

#[derive(Default, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum IpVersion {
    #[serde(alias = "4")]
    V4,
    #[serde(alias = "6")]
    V6,
    #[serde(alias = "46")]
    #[default]
    V46,
}
impl Debug for IpVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                IpVersion::V4 => "4",
                IpVersion::V6 => "6",
                IpVersion::V46 => "46",
            }
        )
    }
}

#[derive(Default, Serialize, Deserialize)]
pub enum AddDel {
    #[default]
    #[serde(alias = "add")]
    Add,
    #[serde(alias = "del")]
    Del,
}

// FIXME block configs that have different set options for the same name
// treat default values as none?

/// User-facing action options
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionOptions {
    /// The set that should be used by this action
    pub set: String,
    /// The pattern name of the IP.
    /// Defaults to "ip"
    #[serde(default = "serde_ip")]
    pub pattern: String,
    #[serde(skip)]
    ip_index: usize,
    // Whether the action is to "add" or "del" the ip from the set
    #[serde(default)]
    action: AddDel,

    #[serde(flatten)]
    pub set_options: SetOptions,
}

fn serde_ip() -> String {
    "ip".into()
}

impl ActionOptions {
    pub fn set_ip_index(&mut self, patterns: Vec<String>) -> Result<(), ()> {
        self.ip_index = patterns
            .into_iter()
            .enumerate()
            .filter(|(_, name)| name == &self.pattern)
            .next()
            .ok_or(())?
            .0;
        Ok(())
    }
}

/// Merged set options
#[derive(Default, Deserialize, Serialize)]
pub struct SetOptions {
    /// The IP type.
    /// Defaults to `46`.
    /// If `4`: creates an IPv4 set with this name
    /// If `6`: creates an IPv6 set with this name
    /// If `46`: creates an IPv4 set with its name suffixed by 'v4' AND an IPv6 set with its name suffixed by 'v6'
    /// *Merged set-wise*.
    #[serde(default)]
    version: Option<IpVersion>,
    /// Chains where the IP set should be inserted.
    /// Defaults to `["INPUT", "FORWARD"]`
    /// *Merged set-wise*.
    #[serde(default)]
    chains: Option<Vec<String>>,
    // Optional timeout, letting linux/netfilter handle set removal instead of reaction
    // Note that `reaction show` and `reaction flush` won't work if set instead of an `after` action
    /// *Merged set-wise*.
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<String>,
    #[serde(skip)]
    timeout_u32: Option<u32>,
    // Target that iptables should use when the IP is encountered.
    // Defaults to DROP, but can also be ACCEPT, RETURN or any user-defined chain
    /// *Merged set-wise*.
    #[serde(default)]
    target: Option<String>,
}

impl SetOptions {
    pub fn merge(&mut self, options: &SetOptions) -> Result<(), String> {
        // merge two Option<T> and fail if there is conflict
        fn inner_merge<T: Eq + Clone + std::fmt::Debug>(
            a: &mut Option<T>,
            b: &Option<T>,
            name: &str,
        ) -> Result<(), String> {
            match (&a, &b) {
                (Some(aa), Some(bb)) => {
                    if aa != bb {
                        return Err(format!(
                            "Conflicting options for {name}: `{aa:?}` and `{bb:?}`"
                        ));
                    }
                }
                (None, Some(_)) => {
                    *a = b.clone();
                }
                _ => (),
            };
            Ok(())
        }

        inner_merge(&mut self.version, &options.version, "version")?;
        inner_merge(&mut self.timeout, &options.timeout, "timeout")?;
        inner_merge(&mut self.chains, &options.chains, "chains")?;
        inner_merge(&mut self.target, &options.target, "target")?;

        if let Some(timeout) = &self.timeout {
            let duration = parse_duration(timeout)
                .map_err(|err| format!("failed to parse timeout: {}", err))?
                .as_secs();
            if duration > u32::MAX as u64 {
                return Err(format!(
                    "timeout is limited to {} seconds (approx {} days)",
                    u32::MAX,
                    49_000
                ));
            }
            self.timeout_u32 = Some(duration as u32);
        }

        Ok(())
    }
}

pub struct Set {
    sets: SetNames,
    chains: Vec<String>,
    timeout: Option<u32>,
    target: String,
}

impl Set {
    pub fn from(name: String, options: SetOptions) -> Self {
        Self {
            sets: SetNames::new(name, options.version),
            timeout: options.timeout_u32,
            target: options.target.unwrap_or("DROP".into()),
            chains: options
                .chains
                .unwrap_or(vec!["INPUT".into(), "FORWARD".into()]),
        }
    }

    pub async fn init(&self, ipset: &mut IpSet) -> Result<(), String> {
        for (set, version) in [
            (&self.sets.ipv4, Version::IPv4),
            (&self.sets.ipv6, Version::IPv6),
        ] {
            if let Some(set) = set {
                // create set
                ipset
                    .order(Order::CreateSet(CreateSet {
                        name: set.clone(),
                        version,
                        timeout: self.timeout,
                    }))
                    .await?;
                // insert set in chains
                for chain in &self.chains {
                    ipset
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

    pub async fn destroy(&self, ipset: &mut IpSet) {
        for (set, version) in [
            (&self.sets.ipv4, Version::IPv4),
            (&self.sets.ipv6, Version::IPv6),
        ] {
            if let Some(set) = set {
                for chain in &self.chains {
                    if let Err(err) = ipset
                        .order(Order::RemoveSet(SetChain {
                            set: set.clone(),
                            chain: chain.clone(),
                            target: self.target.clone(),
                        }))
                        .await
                    {
                        eprintln!(
                            "ERROR while removing {version} set {set} from chain {chain}: {err}"
                        );
                    }
                }
                if let Err(err) = ipset.order(Order::DestroySet(set.clone())).await {
                    eprintln!("ERROR while destroying {version} set {set}: {err}");
                }
            }
        }
    }
}

pub struct SetNames {
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
}

impl SetNames {
    pub fn new(name: String, version: Option<IpVersion>) -> Self {
        Self {
            ipv4: match version {
                Some(IpVersion::V4) => Some(name.clone()),
                Some(IpVersion::V6) => None,
                None | Some(IpVersion::V46) => Some(format!("{}v4", name)),
            },
            ipv6: match version {
                Some(IpVersion::V4) => None,
                Some(IpVersion::V6) => Some(name),
                None | Some(IpVersion::V46) => Some(format!("{}v6", name)),
            },
        }
    }
}

pub struct Action {
    ipset: IpSet,
    rx: remocMpsc::Receiver<Exec>,
    shutdown: ShutdownToken,
    sets: SetNames,
    // index of pattern ip in match vec
    ip_index: usize,
    action: AddDel,
}

impl Action {
    pub fn new(
        ipset: IpSet,
        shutdown: ShutdownToken,
        rx: remocMpsc::Receiver<Exec>,
        options: ActionOptions,
    ) -> Result<Self, String> {
        Ok(Action {
            ipset,
            rx,
            shutdown,
            sets: SetNames::new(options.set, options.set_options.version),
            ip_index: options.ip_index,
            action: options.action,
        })
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
                    eprintln!("ERROR {err}");
                    break;
                }
                // ok
                Some(Ok(Some(exec))) => {
                    if let Err(err) = self.handle_exec(exec).await {
                        eprintln!("ERROR {err}");
                        break;
                    }
                }
            }
        }
        eprintln!("DEBUG Asking for shutdown");
        self.shutdown.ask_shutdown();
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
        let set = match (&self.sets.ipv4, &self.sets.ipv6) {
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
        // add/remove ip to set
        self.ipset
            .order(match self.action {
                AddDel::Add => Order::Add(set.clone(), ip),
                AddDel::Del => Order::Del(set.clone(), ip),
            })
            .await?;
        Ok(())
    }
}
