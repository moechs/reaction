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

    pub fn contains(&self, ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(ip) => self.ipv4.get_lpm(&ip.into()).is_some(),
            IpAddr::V6(ip) => self.ipv6.get_lpm(&ip.into()).is_some(),
        }
    }

    pub fn insert(&mut self, ip: IpNet) -> bool {
        match ip {
            IpNet::V4(ipv4) => self.ipv4.insert(ipv4),
            IpNet::V6(ipv6) => self.ipv6.insert(ipv6),
        }
    }

    pub fn remove(&mut self, ip: &IpNet) -> bool {
        match ip {
            IpNet::V4(ipv4) => self.ipv4.remove(ipv4),
            IpNet::V6(ipv6) => self.ipv6.remove(ipv6),
        }
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
                        // break;
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

        let ip = ip_net_from_str(ipstr)?;
        let mut set = self.set.write().await;

        match self.action {
            AddRemove::Add => {
                set.insert(ip);
            }
            AddRemove::Remove => {
                set.remove(&ip);
            }
        }

        Ok(())
    }
}

fn ip_net_from_str(ipstr: &str) -> Result<IpNet, String> {
    if ipstr.contains('/') {
        ipstr.parse().map_err(|err| format!("{err}"))
    } else {
        let ip: IpAddr = ipstr.parse().map_err(|err| format!("{err}"))?;
        Ok(match ip {
            IpAddr::V4(ip) => IpNet::V4(Ipv4Net::new(ip, 32).unwrap()),
            IpAddr::V6(ip) => IpNet::V6(Ipv6Net::new(ip, 128).unwrap()),
        })
    }
}

#[cfg(test)]
#[test]
fn test_dual_stack_set_contains() {
    let mut set = DualStackSet {
        name: "test".into(),
        ipv4: PrefixSet::new(),
        ipv6: PrefixSet::new(),
    };

    let ip1 = ip_net_from_str("203.0.113.1").unwrap();
    let ipnet1 = ip_net_from_str("198.51.100.0/24").unwrap();
    let ipnet1_1 = ip_net_from_str("198.51.100.1").unwrap();
    let ipnet1_2 = ip_net_from_str("198.51.100.2").unwrap();

    assert!(!set.contains(ip1.addr()));
    assert!(!set.contains(ipnet1_1.addr()));
    assert!(!set.contains(ipnet1_2.addr()));
    assert!(!set.contains(ipnet1.addr()));

    set.insert(ip1.clone());

    assert!(set.contains(ip1.addr()));
    assert!(!set.contains(ipnet1_1.addr()));
    assert!(!set.contains(ipnet1_2.addr()));
    assert!(!set.contains(ipnet1.addr()));

    set.insert(ipnet1.clone());

    assert!(set.contains(ip1.addr()));
    assert!(set.contains(ipnet1_1.addr()));
    assert!(set.contains(ipnet1_2.addr()));
    assert!(set.contains(ipnet1.addr()));

    set.remove(&ip1);

    assert!(!set.contains(ip1.addr()));
    assert!(set.contains(ipnet1_1.addr()));
    assert!(set.contains(ipnet1_2.addr()));
    assert!(set.contains(ipnet1.addr()));

    set.remove(&ipnet1);

    assert!(!set.contains(ip1.addr()));
    assert!(!set.contains(ipnet1_1.addr()));
    assert!(!set.contains(ipnet1_2.addr()));
    assert!(!set.contains(ipnet1.addr()));
}
