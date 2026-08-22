use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use ipnet::{Ipv4Net, Ipv6Net};
use prefix_trie::PrefixSet;

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
