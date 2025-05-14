use std::{collections::BTreeSet, fs::create_dir, io::ErrorKind, marker::PhantomData};

use fjall::{
    PartitionCreateOptions, ReadTransaction, Result, Slice, TransactionalKeyspace, WriteTransaction,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::concepts::{Config, Filter, Match, Time};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MatchTime {
    pub m: Match,
    pub t: Time,
}

pub fn open_db(config: &Config) -> std::result::Result<TransactionalKeyspace, String> {
    let fjall_dir = format!("{}/fjall", config.state_directory());

    match create_dir(&fjall_dir) {
        Ok(_) => Ok(()),
        Err(err) => match err.kind() {
            ErrorKind::AlreadyExists => Ok(()),
            _ => Err(format!(
                "while creating database directory {}: {}",
                config.state_directory(),
                err
            )),
        },
    }?;

    let env = fjall::Config::new(fjall_dir)
        .fsync_ms(Some(1000))
        .open_transactional()
        .map_err(|err| {
            format!(
                "while opening database at {}: {}",
                config.state_directory(),
                err
            )
        })?;

    env.cleanup_unused_dbs(config).map_err(|err| {
        format!(
            "while cleaning database on startup at {}: {}",
            config.state_directory(),
            err
        )
    })?;

    Ok(env)
}

/// This trait permits to manage in a single place what are the names of [`fjall::TransactionalPartitionHandle`]s in
/// reaction. It streamlines [`fjall::TransactionalPartitionHandle`]s opening so that we can reliably open the same
/// Databases in multiple places.
/// It also permits to manage the cleanup of unused databases.
pub trait KeyspaceExt {
    fn open_filter_matches_db(&self, filter: &Filter) -> Result<Tree<Match, BTreeSet<Time>>>;
    fn open_filter_ordered_times_db(&self, filter: &Filter) -> Result<Tree<Time, Match>>;
    fn open_filter_triggers_db(&self, filter: &Filter) -> Result<Tree<MatchTime, usize>>;

    #[allow(dead_code)] // See FIXME
    fn cleanup_unused_dbs(&self, config: &Config) -> Result<()>;
}

fn filter_matches_db_name(filter: &Filter) -> String {
    format!("filter_matches_{}.{}", filter.stream_name(), filter.name())
}

fn filter_ordered_times_db_name(filter: &Filter) -> String {
    format!(
        "filter_ordered_times_{}.{}",
        filter.stream_name(),
        filter.name()
    )
}

fn filter_triggers_db_name(filter: &Filter) -> String {
    format!("filter_triggers_{}.{}", filter.stream_name(), filter.name())
}

impl KeyspaceExt for TransactionalKeyspace {
    fn open_filter_matches_db(&self, filter: &Filter) -> Result<Tree<Match, BTreeSet<Time>>> {
        // FIXME Partition names can be up to 255 characters long, can not be empty and can only contain alphanumerics, underscore (_), dash (-), hash tag (#) and dollar ($).
        self.open_partition(
            &filter_matches_db_name(filter),
            PartitionCreateOptions::default(),
        )
        .map(Tree::new)
    }

    fn open_filter_ordered_times_db(&self, filter: &Filter) -> Result<Tree<Time, Match>> {
        self.open_partition(
            &filter_ordered_times_db_name(filter),
            PartitionCreateOptions::default(),
        )
        .map(Tree::new)
    }

    fn open_filter_triggers_db(&self, filter: &Filter) -> Result<Tree<MatchTime, usize>> {
        self.open_partition(
            &filter_triggers_db_name(filter),
            PartitionCreateOptions::default(),
        )
        .map(Tree::new)
    }

    fn cleanup_unused_dbs(&self, config: &Config) -> Result<()> {
        let valid_db_names: BTreeSet<_> = config
            .streams()
            .values()
            // for each filter
            .flat_map(|stream| stream.filters().values())
            .flat_map(|filter| {
                [
                    filter_matches_db_name(filter),
                    filter_ordered_times_db_name(filter),
                    filter_triggers_db_name(filter),
                ]
            })
            .collect();

        // let rtxn = self.read_tx()?;
        // let unnamed: Tree<Str, DecodeIgnore> = self
        //     .open_database(&rtxn, None)?
        //     .expect("the unnamed database always exists");

        // Remove dbs that are not in the list of valid dbs
        for outdated_db in self
            .list_partitions()
            .iter()
            .filter(|db_name| !valid_db_names.contains(&db_name.to_string()))
        {
            self.delete_partition(
                self.open_partition(outdated_db, PartitionCreateOptions::default())?,
            )?;
        }

        Ok(())
    }
}

/// This [`fjall::TransactionalKeyspace`] wrapper permits to have typed Trees and avoid handling the de/serialization in
/// business logic.
/// Key and value types must be [`serde::Serialize`] and [`serde::Deserialize`].
#[derive(Clone)]
pub struct Tree<K: Serialize + DeserializeOwned, V: Serialize + DeserializeOwned> {
    tree: fjall::TxPartitionHandle,
    _k_marker: PhantomData<K>,
    _v_marker: PhantomData<V>,
}

#[allow(clippy::unwrap_used)]
impl<K: Serialize + DeserializeOwned, V: Serialize + DeserializeOwned> Tree<K, V> {
    fn new(tree: fjall::TxPartitionHandle) -> Self {
        Self {
            tree,
            _k_marker: PhantomData::<K>,
            _v_marker: PhantomData::<V>,
        }
    }

    pub fn get(&self, rtxn: &WriteTransaction, k: &K) -> Result<Option<V>> {
        let k = bincode::serialize(k).unwrap();
        Ok(rtxn
            .get(&self.tree, k)?
            .map(|v| bincode::deserialize(&v).unwrap()))
    }

    pub fn remove(&self, wtxn: &mut WriteTransaction, key: &K) {
        let key = bincode::serialize(key).unwrap();
        wtxn.remove(&self.tree, key);
    }

    pub fn first(&self, wtxn: &WriteTransaction) -> Result<Option<(K, V)>> {
        let option = wtxn.first_key_value(&self.tree)?;
        match option {
            None => Ok(None),
            Some((k, v)) => {
                let k: K = bincode::deserialize(&k).unwrap();
                let v: V = bincode::deserialize(&v).unwrap();
                Ok(Some((k, v)))
            }
        }
    }

    // pub fn pop_min(&self, wtxn: &mut WriteTransaction) -> Result<Option<(K, V)>> {
    //     let option = wtxn.pop_min(self.tree)?;
    //     match option {
    //         None => Ok(None),
    //         Some((k, v)) => {
    //             let k: K = bincode::deserialize(&k).unwrap();
    //             let v: V = bincode::deserialize(&v).unwrap();
    //             Ok(Some((k, v)))
    //         }
    //     }
    // }

    pub fn fetch_update<F>(&self, wtxn: &mut WriteTransaction, k: &K, mut f: F) -> Result<Option<V>>
    where
        F: FnMut(Option<V>) -> Option<V>,
    {
        let k = bincode::serialize(&k).unwrap();
        let f = |v: Option<&Slice>| -> Option<Slice> {
            let v = v.map(|v| bincode::deserialize(v).unwrap());
            f(v).map(|v| bincode::serialize(&v).unwrap().into())
        };
        Ok(wtxn
            .fetch_update(&self.tree, k, f)?
            .map(|v| bincode::deserialize::<V>(&v).unwrap()))
    }

    pub fn insert(&self, wtxn: &mut WriteTransaction, k: &K, v: &V) {
        let k = bincode::serialize(k).unwrap();
        let v = bincode::serialize(v).unwrap();
        wtxn.insert(&self.tree, k, v);
    }

    // The lifetime annotations permit to decouple the lifetime of self
    // from the limetime of the Iterator
    #[allow(clippy::needless_lifetimes)] // I find this clearer with 2 lifetimes
    pub fn iter<'a, 'b>(&'a self, rtxn: &ReadTransaction) -> impl Iterator<Item = (K, V)> + 'b {
        rtxn.iter(&self.tree).map(|elt| {
            let (k, v) = elt.unwrap();
            let k: K = bincode::deserialize(&k).unwrap();
            let v: V = bincode::deserialize(&v).unwrap();
            (k, v)
        })
    }
}
