use std::{fmt::Debug, u32, usize};

use reaction_plugin::{Exec, shutdown::ShutdownToken, time::parse_duration};
use remoc::rch::mpsc as remocMpsc;
use serde::{Deserialize, Serialize};

use crate::ipset::{CreateSet, IpSet, Order, SetChain, Version};

#[derive(Default, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum IpVersion {
    #[default]
    #[serde(rename = "ip")]
    Ip,
    #[serde(rename = "ipv4")]
    Ipv4,
    #[serde(rename = "ipv6")]
    Ipv6,
}
impl Debug for IpVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                IpVersion::Ipv4 => "ipv4",
                IpVersion::Ipv6 => "ipv6",
                IpVersion::Ip => "ip",
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
#[derive(Default, Clone, Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct SetOptions {
    /// The IP type.
    /// Defaults to `46`.
    /// If `ipv4`: creates an IPv4 set with this name
    /// If `ipv6`: creates an IPv6 set with this name
    /// If `ip`: creates an IPv4 set with its name suffixed by 'v4' AND an IPv6 set with its name suffixed by 'v6'
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
    // Same syntax as after and retryperiod in reaction.
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

    pub async fn init(&self, ipset: &mut IpSet) -> Result<(), (usize, String)> {
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
                    .await
                    .map_err(|err| (0, err.to_string()))?;
                // insert set in chains
                for (i, chain) in self.chains.iter().enumerate() {
                    ipset
                        .order(Order::InsertSet(SetChain {
                            set: set.clone(),
                            chain: chain.clone(),
                            target: self.target.clone(),
                        }))
                        .await
                        .map_err(|err| (i + 1, err.to_string()))?;
                }
            }
        }
        Ok(())
    }

    pub async fn destroy(&self, ipset: &mut IpSet, until: Option<usize>) {
        for set in [&self.sets.ipv4, &self.sets.ipv6] {
            if let Some(set) = set {
                for chain in self
                    .chains
                    .iter()
                    .take(until.map(|until| until - 1).unwrap_or(usize::MAX))
                {
                    let _ = ipset
                        .order(Order::RemoveSet(SetChain {
                            set: set.clone(),
                            chain: chain.clone(),
                            target: self.target.clone(),
                        }))
                        .await;
                }
                if until.is_none_or(|until| until != 0) {
                    let _ = ipset.order(Order::DestroySet(set.clone())).await;
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
                Some(IpVersion::Ipv4) => Some(name.clone()),
                Some(IpVersion::Ipv6) => None,
                None | Some(IpVersion::Ip) => Some(format!("{}v4", name)),
            },
            ipv6: match version {
                Some(IpVersion::Ipv4) => None,
                Some(IpVersion::Ipv6) => Some(name),
                None | Some(IpVersion::Ip) => Some(format!("{}v6", name)),
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
        // eprintln!("DEBUG Asking for shutdown");
        // self.shutdown.ask_shutdown();
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

#[cfg(test)]
mod tests {
    use crate::action::{IpVersion, SetOptions};

    #[tokio::test]
    async fn set_options_merge() {
        let s1 = SetOptions {
            version: None,
            chains: None,
            timeout: None,
            timeout_u32: None,
            target: None,
        };
        let s2 = SetOptions {
            version: Some(IpVersion::Ipv4),
            chains: Some(vec!["INPUT".into()]),
            timeout: Some("3h".into()),
            timeout_u32: Some(3 * 3600),
            target: Some("DROP".into()),
        };
        assert_ne!(s1, s2);
        assert_eq!(s1, SetOptions::default());

        {
            // s2 can be merged in s1
            let mut s1 = s1.clone();
            assert!(s1.merge(&s2).is_ok());
            assert_eq!(s1, s2);
        }

        {
            // s1 can be merged in s2
            let mut s2 = s2.clone();
            assert!(s2.merge(&s1).is_ok());
        }

        {
            // s1 can be merged in itself
            let mut s3 = s1.clone();
            assert!(s3.merge(&s1).is_ok());
            assert_eq!(s1, s3);
        }

        {
            // s2 can be merged in itself
            let mut s3 = s2.clone();
            assert!(s3.merge(&s2).is_ok());
            assert_eq!(s2, s3);
        }

        for s3 in [
            SetOptions {
                version: Some(IpVersion::Ipv6),
                ..Default::default()
            },
            SetOptions {
                chains: Some(vec!["damn".into()]),
                ..Default::default()
            },
            SetOptions {
                timeout: Some("30min".into()),
                ..Default::default()
            },
            SetOptions {
                target: Some("log-refuse".into()),
                ..Default::default()
            },
        ] {
            // none with some is ok
            assert!(s3.clone().merge(&s1).is_ok(), "s3: {s3:?}");
            assert!(s1.clone().merge(&s3).is_ok(), "s3: {s3:?}");
            // different some is ko
            assert!(s3.clone().merge(&s2).is_err(), "s3: {s3:?}");
            assert!(s2.clone().merge(&s3).is_err(), "s3: {s3:?}");
        }
    }
}
