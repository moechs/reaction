use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    net::IpAddr,
    sync::Arc,
};

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use prefix_trie::joint::JointPrefixSet;
use reaction_plugin::Exec;
use tokio::sync::RwLock;

use crate::config::{AddDel, ComputedAction};

pub type IpSets = HashMap<String, IpSet>;

#[derive(Clone)]
pub struct IpSet {
    pub name: String,
    inner: Arc<RwLock<JointPrefixSet<IpNet>>>,
}

impl IpSet {
    pub fn new(name: String) -> Self {
        Self {
            name,
            inner: Arc::new(RwLock::new(JointPrefixSet::new())),
        }
    }

    pub async fn contains(&self, ip: IpAddr) -> bool {
        self.inner.read().await.is_covered(&ip.into())
    }

    pub async fn insert(&mut self, ip: IpNet) -> bool {
        self.inner.write().await.insert(ip)
    }

    pub async fn remove(&mut self, ip: &IpNet) -> bool {
        self.inner.write().await.remove(ip)
    }
}

#[cfg(test)]
impl PartialEq for IpSet {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}
#[cfg(test)]
impl Eq for IpSet {}

impl Debug for IpSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpSet").field("name", &self.name).finish()
    }
}

pub fn ip_sets_from_names(names: &HashSet<String>) -> IpSets {
    names
        .iter()
        .map(|set| (set.clone(), IpSet::new(set.clone())))
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

        match self.action {
            AddDel::Add => {
                self.set.insert(ip).await;
            }
            AddDel::Delete => {
                self.set.remove(&ip).await;
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
#[tokio::test]
async fn test_ip_set_contains() {
    let mut set = IpSet::new("test".into());

    let ip1 = ip_net_from_str("203.0.113.1").unwrap();
    let ipnet1 = ip_net_from_str("198.51.100.0/24").unwrap();
    let ipnet1_1 = ip_net_from_str("198.51.100.1").unwrap();
    let ipnet1_2 = ip_net_from_str("198.51.100.2").unwrap();

    assert!(!set.contains(ip1.addr()).await);
    assert!(!set.contains(ipnet1_1.addr()).await);
    assert!(!set.contains(ipnet1_2.addr()).await);
    assert!(!set.contains(ipnet1.addr()).await);

    set.insert(ip1.clone()).await;

    assert!(set.contains(ip1.addr()).await);
    assert!(!set.contains(ipnet1_1.addr()).await);
    assert!(!set.contains(ipnet1_2.addr()).await);
    assert!(!set.contains(ipnet1.addr()).await);

    set.insert(ipnet1.clone()).await;

    assert!(set.contains(ip1.addr()).await);
    assert!(set.contains(ipnet1_1.addr()).await);
    assert!(set.contains(ipnet1_2.addr()).await);
    assert!(set.contains(ipnet1.addr()).await);

    set.remove(&ip1).await;

    assert!(!set.contains(ip1.addr()).await);
    assert!(set.contains(ipnet1_1.addr()).await);
    assert!(set.contains(ipnet1_2.addr()).await);
    assert!(set.contains(ipnet1.addr()).await);

    set.remove(&ipnet1).await;

    assert!(!set.contains(ip1.addr()).await);
    assert!(!set.contains(ipnet1_1.addr()).await);
    assert!(!set.contains(ipnet1_2.addr()).await);
    assert!(!set.contains(ipnet1.addr()).await);
}
