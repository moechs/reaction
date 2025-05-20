use std::{collections::BTreeMap, ops::Deref};

use chrono::{Local, TimeDelta};

use super::{env::Entry, KeyType, ValueType};

/// Main API of this crate.
/// [`BTreeMapWal`] wraps and is meant to be used exactly like a standard [`std::collections::BTreeMap`].
/// Read operations are RAM only.
/// Write operations are asynchronously persisted on disk by its parent [`Database`].
pub struct Tree<K: KeyType, V: ValueType> {
    id: usize,
    entry_timeout: TimeDelta,
    tree: BTreeMap<K, V>,
    // TODO channel::send
}

// Gives access to all read-only functions
impl<K: KeyType, V: ValueType> Deref for Tree<K, V> {
    type Target = BTreeMap<K, V>;

    fn deref(&self) -> &Self::Target {
        &self.tree
    }
}

// Reimplement write functions
impl<K: KeyType, V: ValueType> Tree<K, V> {
    fn log(&mut self, k: K, v: Option<V>) {
        let e = Entry {
            tree: self.id,
            key: serde_json::to_value(k).expect("could not serialize key"),
            value: v.map(|v| serde_json::to_value(v).expect("could not serialize value")),
            expiry: Local::now() + self.entry_timeout,
        };
    }
}
