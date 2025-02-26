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
pub struct Tree<K: Serialize + DeserializeOwned + Ord, V: Serialize + DeserializeOwned> {
    tree: sled::Tree,
    _k_marker: PhantomData<K>,
    _v_marker: PhantomData<V>,
}

#[allow(clippy::unwrap_used)]
impl<K: Serialize + DeserializeOwned + Ord, V: Serialize + DeserializeOwned> Tree<K, V> {
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

    #[cfg(test)]
    pub fn as_map(&self) -> BTreeMap<K, V> {
        self.iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::{Local, TimeDelta};

    use super::SledDbExt;
    use crate::{concepts::filter_tests::ok_filter, tests::TempDb};

    #[test]
    fn tree_crud() {
        let filter = ok_filter();
        let db = TempDb::default();
        let triggers = db.open_filter_triggers_tree(&filter).unwrap();
        assert_eq!(BTreeMap::default(), triggers.as_map());

        let now = Local::now();
        let then = now + TimeDelta::seconds(2);

        let k1 = vec!["a".into()];
        let k2 = vec!["a".into(), "b".into()];

        let v1 = BTreeMap::from([(now, 4)]);
        let v2 = BTreeMap::from([(then, 2)]);

        let map_1 = BTreeMap::from([(k1.clone(), v1.clone())]);
        let map_2 = BTreeMap::from([(k2.clone(), v2.clone())]);
        let map_1_2 = BTreeMap::from([(k1.clone(), v1.clone()), (k2.clone(), v2.clone())]);

        triggers.insert(&k1, &v1).unwrap();
        assert_eq!(triggers.as_map(), map_1);
        assert_eq!(triggers.get(&k1).unwrap(), Some(v1.clone()));
        assert_eq!(triggers.get(&k2).unwrap(), None);

        triggers.insert(&k2, &v2).unwrap();
        assert_eq!(triggers.as_map(), map_1_2);
        assert_eq!(triggers.get(&k1).unwrap(), Some(v1.clone()));
        assert_eq!(triggers.get(&k2).unwrap(), Some(v2.clone()));

        assert_eq!(triggers.remove(&k1), Some(v1.clone()));
        assert_eq!(triggers.as_map(), map_2);
        assert_eq!(triggers.get(&k1).unwrap(), None);
        assert_eq!(triggers.get(&k2).unwrap(), Some(v2.clone()));

        // Add back
        triggers
            .fetch_and_update(&k1, |map| {
                let mut map = map.unwrap_or_default();
                map.insert(now, 4);
                Some(map)
            })
            .unwrap();
        assert_eq!(triggers.as_map(), map_1_2);
        assert_eq!(triggers.get(&k1).unwrap(), Some(v1.clone()));
        assert_eq!(triggers.get(&k2).unwrap(), Some(v2.clone()));

        // Remove
        triggers
            .fetch_and_update(&k1, |map| match map {
                Some(_) => None,
                None => Some(v1.clone()),
            })
            .unwrap();
        assert_eq!(triggers.as_map(), map_2);
        assert_eq!(triggers.get(&k1).unwrap(), None);
        assert_eq!(triggers.get(&k2).unwrap(), Some(v2.clone()));

        // Remove
        triggers.fetch_and_update(&k2, |_| None).unwrap();
        assert_eq!(triggers.as_map(), BTreeMap::default());
        assert_eq!(triggers.get(&k1).unwrap(), None);
        assert_eq!(triggers.get(&k2).unwrap(), None);
    }
}
