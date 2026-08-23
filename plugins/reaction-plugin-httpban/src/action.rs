use std::{
    collections::BTreeMap,
    net::IpAddr,
    sync::{Arc, RwLock},
};

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use prefix_trie::PrefixSet;
use reaction_plugin::Exec;

use crate::config::{AddRemove, ComputedActionOptions};

// TODO if high-latency is detected, we should change to an RwLock from tokio
pub type IpSets = BTreeMap<String, Arc<RwLock<JointSets>>>;

pub struct JointSets {
    ipv4: PrefixSet<Ipv4Net>,
    ipv6: PrefixSet<Ipv6Net>,
}

pub fn ip_sets_from_names(names: impl Iterator<Item = String>) -> IpSets {
    names
        .map(|set| {
            (
                set,
                Arc::new(RwLock::new(JointSets {
                    ipv4: PrefixSet::new(),
                    ipv6: PrefixSet::new(),
                })),
            )
        })
        .collect()
}

impl ComputedActionOptions {
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
                    if let Err(err) = self.handle_exec(exec) {
                        eprintln!("ERROR {err}");
                        break;
                    }
                }
            }
        }
    }

    fn handle_exec(&mut self, exec: Exec) -> Result<(), String> {
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

        let mut set = self
            .set
            .write()
            .map_err(|_| "ERROR action: part of the process has crashed, quitting")?;

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
