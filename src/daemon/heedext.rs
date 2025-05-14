use std::{collections::BTreeSet, fs::create_dir, io::ErrorKind, marker::PhantomData};

use fjall::{Result, TransactionalKeyspace, WriteTransaction};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::concepts::{Config, Filter, Match, Time};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

/// This trait permits to manage in a single place what are the names of [`heed::Database`]s in
/// reaction. It streamlines [`heed::Database`]s opening so that we can reliably open the same
/// Databases in multiple places.
/// It also permits to manage the cleanup of unused databases.
pub trait EnvExt {
    fn open_filter_matches_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<Match>, SerdeBincode<Time>>>;
    fn open_filter_ordered_times_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<Time>, SerdeBincode<Match>>>;
    fn open_filter_triggers_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<MatchTime>, U64<LittleEndian>>>;

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

impl EnvExt for TransactionalKeyspace {
    fn open_filter_matches_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<Match>, SerdeBincode<Time>>> {
        let mut wtxn = self.write_txn()?;
        let res = self
            .database_options()
            .name(&filter_matches_db_name(filter))
            // We allow duplicates on this database
            .flags(DatabaseFlags::DUP_SORT)
            .types::<SerdeBincode<Match>, SerdeBincode<Time>>()
            .create(&mut wtxn);
        wtxn.commit()?;
        res
    }

    fn open_filter_ordered_times_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<Time>, SerdeBincode<Match>>> {
        let mut wtxn = self.write_txn()?;
        let res = self.create_database(&mut wtxn, Some(&filter_ordered_times_db_name(filter)));
        wtxn.commit()?;
        res
    }

    fn open_filter_triggers_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<MatchTime>, U64<LittleEndian>>> {
        let mut wtxn = self.write_txn()?;
        let res = self.create_database::<SerdeBincode<MatchTime>, U64<LittleEndian>>(
            &mut wtxn,
            Some(&filter_triggers_db_name(filter)),
        );
        wtxn.commit()?;
        res
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
            // convert as IVec which is sled's binary type
            // .map(|string| sled::IVec::from(string.as_bytes()))
            .collect();

        let rtxn = self.read_txn()?;
        let unnamed: Database<Str, DecodeIgnore> = self
            .open_database(&rtxn, None)?
            .expect("the unnamed database always exists");

        let mut wtxn = self.write_txn()?;
        // Remove dbs that are not in the list of valid dbs
        for outdated_db in unnamed
            .iter(&rtxn)?
            .filter(|key| key.is_ok())
            // unwrap because we only kept `Result::Ok`s
            .map(|key| key.unwrap().0)
            .filter(|db_name| !valid_db_names.contains(*db_name))
        {
            if let Some(db) =
                self.open_database::<DecodeIgnore, DecodeIgnore>(&wtxn, Some(outdated_db))?
            {
                // FIXME Waiting for this PR to land in a stable release
                // https://github.com/meilisearch/heed/pull/330
                // For now we're just clearing it
                db.clear(&mut wtxn)?;
                // db.remove(&mut write_txn)?
            }
        }

        wtxn.commit()?;
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

    pub fn get(&self, k: &K) -> Result<Option<V>> {
        let k = bincode::serialize(k).unwrap();
        Ok(self.tree.get(k)?.map(|v| bincode::deserialize(&v).unwrap()))
    }

    pub fn remove(&self, key: &K, wtxn: WriteTransaction) {
        let key = bincode::serialize(key).unwrap();
        wtxn.remove(self, key);
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
