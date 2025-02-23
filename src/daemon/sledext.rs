use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
};

use serde::{de::DeserializeOwned, Serialize};

use sled::Result;

use crate::concepts::{Config, Filter, Match, Time};

/// This trait permits to manage in a single place what are the names of [`sled::Tree`]s in
/// reaction. It streamlines [`sled::Tree`]s opening so that we can reliably open the same Trees in
/// multiple places.
/// It also permits to manage the cleanup of unused trees.
pub trait SledDbExt {
    fn open_filter_matches_tree(&self, filter: &Filter) -> Result<Tree<Match, BTreeSet<Time>>>;
    fn open_filter_ordered_times_tree(&self, filter: &Filter) -> Result<Tree<Time, Match>>;
    fn open_filter_triggers_tree(
        &self,
        filter: &Filter,
    ) -> Result<Tree<Match, BTreeMap<Time, usize>>>;

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
    fn open_filter_matches_tree(&self, filter: &Filter) -> Result<Tree<Match, BTreeSet<Time>>> {
        self.open_tree(filter_matches_tree_name(filter).as_bytes())
            .map(Tree::new)
    }

    fn open_filter_ordered_times_tree(&self, filter: &Filter) -> Result<Tree<Time, Match>> {
        self.open_tree(filter_ordered_times_tree_name(filter).as_bytes())
            .map(Tree::new)
    }

    fn open_filter_triggers_tree(
        &self,
        filter: &Filter,
    ) -> Result<Tree<Match, BTreeMap<Time, usize>>> {
        self.open_tree(filter_triggers_tree_name(filter).as_bytes())
            .map(Tree::new)
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

/// This [`sled::Tree`] wrapper permits to have typed Trees and avoid handling the de/serialization in
/// business logic.
/// Key and value types must be [`serde::Serialize`] and [`serde::Deserialize`].
#[derive(Clone)]
pub struct Tree<K: Serialize + DeserializeOwned, V: Serialize + DeserializeOwned> {
    tree: sled::Tree,
    _k_marker: PhantomData<K>,
    _v_marker: PhantomData<V>,
}

#[allow(clippy::unwrap_used)]
impl<K: Serialize + DeserializeOwned, V: Serialize + DeserializeOwned> Tree<K, V> {
    fn new(tree: sled::Tree) -> Self {
        Self {
            tree,
            _k_marker: PhantomData::<K>,
            _v_marker: PhantomData::<V>,
        }
    }

    pub fn get(&self, k: &K) -> Result<Option<V>> {
        let k = bincode::serialize(k).unwrap();
        Ok(self.tree.get(k)?.map(|v| bincode::deserialize(&v).unwrap()))
    }

    pub fn remove(&self, key: &K) -> Option<V> {
        let key = bincode::serialize(key).unwrap();
        self.tree
            .remove(key)
            .unwrap()
            .map(|value| bincode::deserialize(&value).unwrap())
    }

    pub fn first(&self) -> Result<Option<(K, V)>> {
        let option = self.tree.first()?;
        match option {
            None => Ok(None),
            Some((k, v)) => {
                let k: K = bincode::deserialize(&k).unwrap();
                let v: V = bincode::deserialize(&v).unwrap();
                Ok(Some((k, v)))
            }
        }
    }

    pub fn pop_min(&self) -> Result<Option<(K, V)>> {
        let option = self.tree.pop_min()?;
        match option {
            None => Ok(None),
            Some((k, v)) => {
                let k: K = bincode::deserialize(&k).unwrap();
                let v: V = bincode::deserialize(&v).unwrap();
                Ok(Some((k, v)))
            }
        }
    }

    pub fn fetch_and_update<F>(&self, k: &K, mut f: F) -> Result<Option<V>>
    where
        F: FnMut(Option<V>) -> Option<V>,
    {
        let k = bincode::serialize(&k).unwrap();
        let f = |v: Option<&[u8]>| -> Option<Vec<u8>> {
            let v = v.map(|v| bincode::deserialize(v).unwrap());
            f(v).map(|v| bincode::serialize(&v).unwrap())
        };
        Ok(self
            .tree
            .fetch_and_update(k, f)?
            .map(|v| bincode::deserialize::<V>(&v).unwrap()))
    }

    pub fn insert(&self, k: &K, v: &V) -> Result<Option<V>> {
        let k = bincode::serialize(k).unwrap();
        let v = bincode::serialize(v).unwrap();
        Ok(self
            .tree
            .insert(k, v)?
            .map(|v| bincode::deserialize(&v).unwrap()))
    }

    // The lifetime annotations permit to decouple the lifetime of self
    // from the limetime of the Iterator
    #[allow(clippy::needless_lifetimes)] // I find this clearer with 2 lifetimes
    pub fn iter<'a, 'b>(&'a self) -> impl Iterator<Item = (K, V)> + 'b {
        self.tree.iter().map(|elt| {
            let (k, v) = elt.unwrap();
            let k: K = bincode::deserialize(&k).unwrap();
            let v: V = bincode::deserialize(&v).unwrap();
            (k, v)
        })
    }
}
