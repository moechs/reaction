use std::collections::BTreeSet;

use serde::{de::DeserializeOwned, Serialize};

use sled::Result;

use crate::concepts::{Config, Filter};

/// This trait permits to streamline how to open the correct [`Tree`]s
/// so that we can reliably open the same Trees in multiple places
///
/// [`Tree`]: https://docs.rs/sled/latest/sled/struct.Tree.html
pub trait SledDbExt {
    fn open_filter_matches_tree(&self, filter: &Filter) -> Result<sled::Tree>;
    fn open_filter_ordered_times_tree(&self, filter: &Filter) -> Result<sled::Tree>;
    fn open_filter_triggers_tree(&self, filter: &Filter) -> Result<sled::Tree>;
    fn cleanup_unused_trees(&self, config: &Config);
}

fn filter_matches_tree_name(filter: &Filter) -> String {
    format!("filter_matches_{}.{}", filter.stream_name(), filter.name())
}

fn filter_ordered_times_tree_name(filter: &Filter) -> String {
    format!(
        "filter_ordered_times_{}.{}",
        filter.stream_name(),
        filter.name()
    )
}

fn filter_triggers_tree_name(filter: &Filter) -> String {
    format!("filter_triggers_{}.{}", filter.stream_name(), filter.name())
}

impl SledDbExt for sled::Db {
    fn open_filter_matches_tree(&self, filter: &Filter) -> Result<sled::Tree> {
        self.open_tree(filter_matches_tree_name(filter).as_bytes())
    }
    fn open_filter_ordered_times_tree(&self, filter: &Filter) -> Result<sled::Tree> {
        self.open_tree(filter_ordered_times_tree_name(filter).as_bytes())
    }
    fn open_filter_triggers_tree(&self, filter: &Filter) -> Result<sled::Tree> {
        self.open_tree(filter_triggers_tree_name(filter).as_bytes())
    }

    fn cleanup_unused_trees(&self, config: &Config) {
        let valid_tree_names: BTreeSet<_> = config
            .streams()
            .values()
            // for each filter
            .flat_map(|stream| stream.filters().values())
            .flat_map(|filter| {
                [
                    filter_matches_tree_name(filter),
                    filter_ordered_times_tree_name(filter),
                    filter_triggers_tree_name(filter),
                ]
            })
            // plus sled's default map
            .chain(std::iter::once("__sled__default".into()))
            // convert as IVec which is sled's binary type
            .map(|string| sled::IVec::from(string.as_bytes()))
            .collect();

        // Remove trees that are not in the list of valid trees
        for outdated_tree in self
            .tree_names()
            .into_iter()
            .filter(|tree_name| !valid_tree_names.contains(tree_name))
        {
            self.drop_tree(outdated_tree)
                .expect("Fatal error while cleaning DB on startup");
        }
    }
}

/// This trait just permits to have less verbose typing in the next trait
pub trait KV: Serialize + DeserializeOwned {}
impl<T> KV for T where T: Serialize + DeserializeOwned {}

/// This trait permits to have typed Trees and avoid handling the de/serialization in business logic.
/// key and value types must be [`Serialize`] and [`Deserialize`].
///
/// [`Serialize`]: https://docs.rs/serde/latest/serde/trait.Serialize.html
/// [`Deserialize`]: https://docs.rs/serde/latest/serde/trait.Deserialize.html
pub trait SledTreeExt {
    fn get_ext<K: KV, V: KV>(&self, k: &K) -> Result<Option<V>>;
    fn remove_ext<K: KV, V: KV>(&self, k: &K) -> Option<V>;
    fn first_ext<K: KV, V: KV>(&self) -> Result<Option<(K, V)>>;
    fn pop_min_ext<K: KV, V: KV>(&self) -> Result<Option<(K, V)>>;
    fn fetch_and_update_ext<K: KV, V: KV, F>(&self, k: &K, f: F) -> Result<Option<V>>
    where
        F: FnMut(Option<V>) -> Option<V>;
    fn insert_ext<K: KV, V: KV>(&self, k: &K, v: &V) -> Result<Option<V>>;
    #[allow(clippy::needless_lifetimes)] // I find this clearer with 2 lifetimes
    fn iter_ext<'a, 'b, K: KV, V: KV>(&'a self) -> impl Iterator<Item = (K, V)> + 'b;
}

#[allow(clippy::unwrap_used)]
impl SledTreeExt for sled::Tree {
    fn get_ext<K: KV, V: KV>(&self, k: &K) -> Result<Option<V>> {
        let k = bincode::serialize(k).unwrap();
        Ok(self.get(k)?.map(|v| bincode::deserialize(&v).unwrap()))
    }

    fn remove_ext<K: KV, V: KV>(&self, key: &K) -> Option<V> {
        let key = bincode::serialize(key).unwrap();
        self.remove(key)
            .unwrap()
            .map(|value| bincode::deserialize(&value).unwrap())
    }

    fn first_ext<K: KV, V: KV>(&self) -> Result<Option<(K, V)>> {
        let option = self.first()?;
        match option {
            None => Ok(None),
            Some((k, v)) => {
                let k: K = bincode::deserialize(&k).unwrap();
                let v: V = bincode::deserialize(&v).unwrap();
                Ok(Some((k, v)))
            }
        }
    }

    fn pop_min_ext<K: KV, V: KV>(&self) -> Result<Option<(K, V)>> {
        let option = self.pop_min()?;
        match option {
            None => Ok(None),
            Some((k, v)) => {
                let k: K = bincode::deserialize(&k).unwrap();
                let v: V = bincode::deserialize(&v).unwrap();
                Ok(Some((k, v)))
            }
        }
    }

    fn fetch_and_update_ext<K: KV, V: KV, F>(&self, k: &K, mut f: F) -> Result<Option<V>>
    where
        F: FnMut(Option<V>) -> Option<V>,
    {
        let k = bincode::serialize(&k).unwrap();
        let f = |v: Option<&[u8]>| -> Option<Vec<u8>> {
            let v = v.map(|v| bincode::deserialize(v).unwrap());
            f(v).map(|v| bincode::serialize(&v).unwrap())
        };
        Ok(self
            .fetch_and_update(k, f)?
            .map(|v| bincode::deserialize::<V>(&v).unwrap()))
    }

    fn insert_ext<K: KV, V: KV>(&self, k: &K, v: &V) -> Result<Option<V>> {
        let k = bincode::serialize(k).unwrap();
        let v = bincode::serialize(v).unwrap();
        Ok(self
            .insert(k, v)?
            .map(|v| bincode::deserialize(&v).unwrap()))
    }

    // The lifetime annotations permit to decouple the lifetime of self
    // from the limetime of the Iterator
    #[allow(clippy::needless_lifetimes)] // I find this clearer with 2 lifetimes
    fn iter_ext<'a, 'b, K: KV, V: KV>(&'a self) -> impl Iterator<Item = (K, V)> + 'b {
        self.iter().map(|elt| {
            let (k, v) = elt.unwrap();
            let k: K = bincode::deserialize(&k).unwrap();
            let v: V = bincode::deserialize(&v).unwrap();
            (k, v)
        })
    }
}
