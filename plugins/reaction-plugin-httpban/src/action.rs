use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    net::IpAddr,
    sync::Arc,
};

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use prefix_trie::PrefixSet;
use reaction_plugin::Exec;
use tokio::sync::RwLock;

use crate::config::{AddRemove, ComputedAction};

pub type IpSets = HashMap<String, IpSet>;
pub type IpSet = Arc<RwLock<DualStackSet>>;

pub struct DualStackSet {
    pub name: String,
    ipv4: PrefixSet<Ipv4Net>,
    ipv6: PrefixSet<Ipv6Net>,
}

impl DualStackSet {
    pub fn new(name: String) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(Self {
            name,
            ipv4: PrefixSet::new(),
            ipv6: PrefixSet::new(),
        }))
    }
}

impl Debug for DualStackSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JointSets")
            .field("name", &self.name)
            .finish()
    }
}

pub fn ip_sets_from_names(names: &HashSet<String>) -> IpSets {
    names
        .iter()
        .map(|set| (set.clone(), DualStackSet::new(set.clone())))
        .collect()
}

impl ComputedAction {
    pub async fn serve(mut self) {
        loop {
            match self.rx.recv().await {
                // channel closed
                Ok(None) => break,
                // error from channel
                Err(err) => {
                    eprintln!("ERROR {err}");
                    break;
                }
                // ok
                Ok(Some(exec)) => {
                    if let Err(err) = self.handle_exec(exec).await {
                        eprintln!("ERROR {err}");
                        break;
                    }
                }
            }
        }
    }

    async fn handle_exec(&mut self, exec: Exec) -> Result<(), String> {
        let ipstr = exec
            .match_
            .get(self.pattern)
            .ok_or_else(|| "ERROR action: bad struct from reaction, this is a bug")?;

        let ip: IpNet = if ipstr.contains('/') {
            ipstr.parse().map_err(|err| format!("{err}"))?
        } else {
            let ip: IpAddr = ipstr.parse().map_err(|err| format!("{err}"))?;
            match ip {
                IpAddr::V4(ip) => IpNet::V4(Ipv4Net::new(ip, 32).unwrap()),
                IpAddr::V6(ip) => IpNet::V6(Ipv6Net::new(ip, 128).unwrap()),
            }
        };

        let mut set = self.set.write().await;

        match ip {
            IpNet::V4(ipv4) => match self.action {
                AddRemove::Add => {
                    set.ipv4.insert(ipv4);
                }
                AddRemove::Remove => {
                    set.ipv4.remove(&ipv4);
                }
            },
            IpNet::V6(ipv6) => match self.action {
                AddRemove::Add => {
                    set.ipv6.insert(ipv6);
                }
                AddRemove::Remove => {
                    set.ipv6.remove(&ipv6);
                }
            },
        }

        Ok(())
    }
}
