/// This module implements an asynchronously persisted BTreeMap named [`Tree`],
/// via an unique "Write Behind Log" (in opposition to WAL, "Write Ahead Log").
///
/// This permits to have RAM-speed read & write operations, while eventually
/// persisting operations. The log is flushed to kernel every 2 seconds.
///
/// Operations stored in the log have a timeout configured at the Tree level.
/// All operations are then stored for this lifetime.
///
/// Data is stored as JSONL. Each line stores a (key, value), plus the tree id,
/// and an expiry timestamp in milliseconds.
use std::{
    collections::{BTreeMap, HashMap},
    io::{Error as IoError, ErrorKind},
    ops::Deref,
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{Local, TimeDelta};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    fs::{rename, File},
    sync::{mpsc, oneshot},
    time::{interval, MissedTickBehavior},
};

use crate::{
    concepts::{Config, Time},
    daemon::ShutdownToken,
};

pub mod helpers;

// Database

use raw::{ReadDB, WriteDB};

mod raw;

/// Entry sent from [`Tree`] to [`Database`]
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub tree: String,
    pub key: Value,
    pub value: Option<Value>,
    pub expiry: Time,
}

pub type LoadedDB = HashMap<String, HashMap<Value, Value>>;

const DB_NAME: &str = "reaction.db";
const DB_NEW_NAME: &str = "reaction.new.db";

impl Config {
    fn path_of(&self, name: &str) -> PathBuf {
        if self.state_directory().is_empty() {
            name.into()
        } else {
            PathBuf::from(self.state_directory()).join(name)
        }
    }
}

// TODO rotate_db at a regular interval instead of every N bytes?
// This would make more sense, as actual garbage collection is time-based

/// A [`Database`] logs all write operations on [`Tree`]s in a single file.
/// Logs are written asynchronously, so the write operations in RAM will never block.
pub struct Database {
    /// Inner database
    write_db: WriteDB,
    /// [`Tree`]s loaded from disk
    loaded_db: LoadedDB,
    /// Path for the "normal" database
    path: PathBuf,
    /// Path for the "new" database, when rotating database.
    /// New database atomically replaces the old one when its writing is done.
    new_path: PathBuf,
    /// The receiver on [`Tree`] write operations
    entry_rx: mpsc::Receiver<Entry>,
    /// The sender on [`Tree`] write operations.
    /// Only used to clone new senders for new Trees.
    entry_tx: mpsc::Sender<Entry>,
    /// The interval at which the database must be flushed to kernel
    flush_every: Duration,
    /// The maximum bytes that must be written until the database is rotated
    max_bytes: usize,
    /// Counter to account of the current number of bytes written
    bytes_written: usize,
}

impl Database {
    pub async fn open(config: &Config) -> Result<Database, IoError> {
        let path = config.path_of(DB_NAME);
        let new_path = config.path_of(DB_NEW_NAME);

        let (write_db, loaded_db) = rotate_db(&path, &new_path, true).await?;

        let (entry_tx, entry_rx) = mpsc::channel(1000);

        Ok(Database {
            write_db,
            loaded_db,
            path,
            new_path,
            entry_rx,
            entry_tx,
            flush_every: Duration::from_secs(2),
            max_bytes: 20 * 1024 * 1024, // 20 MiB
            bytes_written: 0,
        })
    }

    pub fn manager(mut self, shutdown: ShutdownToken) -> oneshot::Receiver<Result<(), String>> {
        let (error_tx, error_rx) = oneshot::channel();
        tokio::spawn(async move {
            let mut interval = interval(self.flush_every);
            // If we missed a tick, it will tick immediately, then wait
            // flush_every for the next tick, resulting in a relaxed interval.
            // Hoping this will smooth IO pressure when under heavy load.
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            let status = loop {
                tokio::select! {
                    entry = self.entry_rx.recv() => {
                        if let Err(err) = self.handle_entry(entry).await {
                            shutdown.ask_shutdown();
                            break err;
                        }
                    }
                    _ = interval.tick() => {
                        if let Err(err) = self.flush().await {
                            shutdown.ask_shutdown();
                            break Some(err);
                        }
                    }
                    _ = shutdown.wait() => break None
                };
            };

            // Shutdown
            let close_status = self
                .close()
                .await
                .map_err(|err| format!("while closing database: {err}"));

            let _ = error_tx.send(if let Some(err) = status {
                Err(err)
            } else if close_status.is_err() {
                close_status
            } else {
                Ok(())
            });
        });
        error_rx
    }

    /// Write a received entry. Return:
    /// - Err(Some) if there was an error,
    /// - Err(None) is channel is closed,
    /// - Ok(()) in general case.
    async fn handle_entry(&mut self, entry: Option<Entry>) -> Result<(), Option<String>> {
        match entry {
            Some(entry) => match self.write_db.write_entry(&entry).await {
                Ok(bytes_written) => {
                    self.bytes_written += bytes_written;
                    if self.bytes_written > self.max_bytes {
                        self.rotate_db()
                            .await
                            .and_then(|_| Ok(self.bytes_written = 0))
                            .map_err(|err| Some(format!("while rotating database: {err}")))
                    } else {
                        Ok(())
                    }
                }
                Err(err) => Err(Some(format!("while writing entry to database: {err}"))),
            },
            None => Err(None),
        }
    }

    /// Flush inner database.
    async fn flush(&mut self) -> Result<(), String> {
        self.write_db
            .flush()
            .await
            .map_err(|err| format!("while flushing database: {err}"))
    }

    /// Close inner database.
    async fn close(mut self) -> Result<(), IoError> {
        self.write_db.close().await
    }

    /// Rotate inner database.
    async fn rotate_db(&mut self) -> Result<(), IoError> {
        self.write_db.close().await?;
        let (write_db, _) = rotate_db(&self.path, &self.new_path, false).await?;
        self.write_db = write_db;
        Ok(())
    }
}

async fn rotate_db(
    path: &Path,
    new_path: &Path,
    startup: bool,
) -> Result<(WriteDB, LoadedDB), IoError> {
    let file = match File::open(&path).await {
        Ok(file) => file,
        Err(err) => match (startup, err.kind()) {
            // No need to rotate the database when it is new,
            // we return here
            (true, ErrorKind::NotFound) => {
                return Ok((WriteDB::new(File::create(path).await?), HashMap::default()))
            }
            (_, _) => return Err(err),
        },
    };

    let mut read_db = ReadDB::new(file);
    let mut write_db = WriteDB::new(File::create(new_path).await?);

    let loaded_db = read_db.read(&mut write_db, startup).await?;

    rename(new_path, path).await?;

    Ok((write_db, loaded_db))
}

// Tree

pub trait KeyType: Ord + Serialize + DeserializeOwned + Clone {}
pub trait ValueType: Ord + Serialize + DeserializeOwned + Clone {}

impl<T> KeyType for T where T: Ord + Serialize + DeserializeOwned + Clone {}
impl<T> ValueType for T where T: Ord + Serialize + DeserializeOwned + Clone {}

/// Main API of this crate.
/// [`Tree`] wraps and is meant to be used exactly like a standard [`std::collections::BTreeMap`].
/// Read operations are RAM only.
/// Write operations are asynchronously persisted on disk by its parent [`Database`].
/// They will never block.
pub struct Tree<K: KeyType, V: ValueType> {
    // FIXME implement id as a u64 instead?
    // Could permit to send more direct database entries.
    // Database should write special entries as soon as a tree is opened.
    /// The name of the tree
    id: String,
    /// The duration for which the data in the tree must be persisted to disk.
    /// All write operations will stay logged on disk for this duration.
    /// This property permits the database rotation to be `O(n)` in time and `O(1)` in RAM space,
    /// `n` being the number of write operations from the last rotation plus the number of new
    /// operations.
    entry_timeout: TimeDelta,
    /// The inner BTreeMap
    tree: BTreeMap<K, V>,
    /// The sender that permits to asynchronously send write operations to database
    tx: mpsc::Sender<Entry>,
}

impl Database {
    /// Creates a new Tree with the given name and entry timeout.
    /// Takes a closure (or regular function) that converts (Value, Value) JSON entries
    /// into (K, V) typed entries.
    /// Helpers for this closure can be find in the [`helpers`] module.
    pub fn open_tree<K: KeyType, V: ValueType, F>(
        &mut self,
        name: String,
        entry_timeout: TimeDelta,
        map_f: F,
    ) -> Result<Tree<K, V>, String>
    where
        F: Fn((Value, Value)) -> Result<(K, V), String>,
    {
        // Load the tree from its JSON
        let tree = if let Some(json_tree) = self.loaded_db.remove(&name) {
            json_tree
                .into_iter()
                .map(map_f)
                .collect::<Result<BTreeMap<K, V>, String>>()
                .unwrap()
        } else {
            BTreeMap::default()
        };
        Ok(Tree {
            id: name,
            entry_timeout,
            tree,
            tx: self.entry_tx.clone(),
        })
    }

    // TODO keep only tree names, and use it for next db rotation to remove associated entries
    /// Drops Trees that have not been loaded already
    pub fn drop_trees(&mut self) {
        self.loaded_db = HashMap::default();
    }
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
    /// Log an [`Entry`] to the [`Database`]
    fn log(&mut self, k: &K, v: Option<&V>) {
        let e = Entry {
            tree: self.id.clone(),
            key: serde_json::to_value(k).expect("could not serialize key"),
            value: v.map(|v| serde_json::to_value(v).expect("could not serialize value")),
            expiry: Local::now() + self.entry_timeout,
        };
        let tx = self.tx.clone();
        // FIXME what if send fails?
        tokio::spawn(async move {
            let _ = tx.send(e).await;
        });
    }

    /// Asynchronously persisted version of [`BTreeMap::insert`]
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.log(&key, Some(&value));
        self.tree.insert(key, value)
    }

    /// Asynchronously persisted version of [`BTreeMap::pop_first`]
    pub fn pop_first(&mut self) -> Option<(K, V)> {
        self.tree.pop_first().map(|(key, value)| {
            self.log(&key, None);
            (key, value)
        })
    }

    /// Asynchronously persisted version of [`BTreeMap::pop_last`]
    pub fn pop_last(&mut self) -> Option<(K, V)> {
        self.tree.pop_last().map(|(key, value)| {
            self.log(&key, None);
            (key, value)
        })
    }

    /// Asynchronously persisted version of [`BTreeMap::remove`]
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.log(&key, None);
        self.tree.remove(key)
    }

    /// Updates an item and returns the previous value.
    /// Returning None removes the item if it existed before.
    /// Asynchronously persisted.
    /// *API design borrowed from [`fjall::WriteTransaction::fetch_update`].*
    pub fn fetch_update<F: FnMut(Option<&V>) -> Option<V>>(
        &mut self,
        key: K,
        mut f: F,
    ) -> Option<V> {
        let old_value = self.get(&key);
        let new_value = f(old_value);
        if old_value != new_value.as_ref() {
            self.log(&key, new_value.as_ref());
        }
        if let Some(new_value) = new_value {
            self.tree.insert(key, new_value)
        } else {
            self.tree.remove(&key)
        }
    }
}

#[cfg(test)]
mod tests {

    use std::{
        collections::{BTreeMap, BTreeSet, HashMap},
        io::Error as IoError,
        path::Path,
    };

    use chrono::{Local, TimeDelta};
    use serde_json::Value;
    use tempfile::{NamedTempFile, TempDir};
    use tokio::fs::{write, File};

    use crate::concepts::Config;

    use super::{
        helpers::*, raw::WriteDB, rotate_db, Database, Entry, KeyType, LoadedDB, Tree, ValueType,
        DB_NAME,
    };

    impl Database {
        pub async fn from_dir(dir_path: &Path) -> Result<Self, IoError> {
            let config_path = dir_path.join("reaction.jsonnet");
            write(
                &config_path,
                format!(
                    "
        {{
            state_directory: {dir_path:?},
            patterns: {{ pattern: {{ regex: \"prout\" }} }},
            streams: {{ dummy: {{
                    cmd: [\"dummy\"],
                    filters: {{ dummy: {{
                            regex: [\"dummy\"],
                            actions: {{ dummy: {{
                                cmd: [\"dummy\"]
                            }} }}
                    }} }}
            }} }}
        }}
        "
                ),
            )
            .await?;

            let config = Config::from_path(&config_path).unwrap();
            Database::open(&config).await
        }

        pub fn set_loaded_db(&mut self, loaded_db: LoadedDB) {
            self.loaded_db = loaded_db;
        }
    }

    impl<K: KeyType, V: ValueType> Tree<K, V> {
        pub fn tree(&self) -> &BTreeMap<K, V> {
            &self.tree
        }
    }

    #[tokio::test]
    async fn test_rotate_db() {
        let now = Local::now();

        let expired = now - TimeDelta::seconds(2);
        let valid = now + TimeDelta::seconds(2);

        let entries = [
            Entry {
                tree: "tree1".into(),
                key: "key1".into(),
                value: Some("value1".into()),
                expiry: valid,
            },
            Entry {
                tree: "tree2".into(),
                key: "key2".into(),
                value: Some("value2".into()),
                expiry: valid,
            },
            Entry {
                tree: "tree1".into(),
                key: "key2".into(),
                value: Some("value2".into()),
                expiry: valid,
            },
            Entry {
                tree: "tree1".into(),
                key: "key1".into(),
                value: None,
                expiry: valid,
            },
            Entry {
                tree: "tree3".into(),
                key: "key1".into(),
                value: Some("value1".into()),
                expiry: expired,
            },
        ];

        let path = NamedTempFile::new().unwrap().into_temp_path();
        let new_path = NamedTempFile::new().unwrap().into_temp_path();

        let mut write_db = WriteDB::new(File::create(&path).await.unwrap());
        for entry in entries {
            write_db.write_entry(&entry).await.unwrap();
        }
        write_db.close().await.unwrap();

        let (mut write_db, loaded_db) = rotate_db(&path, &new_path, true).await.unwrap();
        assert_eq!(
            loaded_db,
            HashMap::from([
                (
                    "tree1".into(),
                    HashMap::from([("key2".into(), "value2".into())])
                ),
                (
                    "tree2".into(),
                    HashMap::from([("key2".into(), "value2".into())])
                )
            ])
        );

        // Test that we can write in new db
        write_db
            .write_entry(&Entry {
                tree: "tree3".into(),
                key: "key3".into(),
                value: Some("value3".into()),
                expiry: valid,
            })
            .await
            .unwrap();
        write_db.close().await.unwrap();

        // And that we get the correct result
        let (_, loaded_db) = rotate_db(&path, &new_path, true).await.unwrap();
        assert_eq!(
            loaded_db,
            HashMap::from([
                (
                    "tree1".into(),
                    HashMap::from([("key2".into(), "value2".into())])
                ),
                (
                    "tree2".into(),
                    HashMap::from([("key2".into(), "value2".into())])
                ),
                (
                    "tree3".into(),
                    HashMap::from([("key3".into(), "value3".into())])
                ),
            ])
        );

        // Test that asking not to load results in no load
        let (_, loaded_db) = rotate_db(&path, &new_path, false).await.unwrap();
        assert_eq!(loaded_db, HashMap::default());
    }

    #[tokio::test]
    async fn test_open_tree() {
        let now = Local::now();
        let now2 = now + TimeDelta::milliseconds(2);
        let now3 = now + TimeDelta::milliseconds(3);

        let now_ms = now.to_rfc3339();
        let now2_ms = now2.to_rfc3339();
        let now3_ms = now3.to_rfc3339();

        let valid = now + TimeDelta::seconds(2);

        let ip127 = vec!["127.0.0.1".to_string()];
        let ip1 = vec!["1.1.1.1".to_string()];

        let entries = [
            Entry {
                tree: "time-match".into(),
                key: now_ms.clone().into(),
                value: Some(ip127.clone().into()),
                expiry: valid,
            },
            Entry {
                tree: "time-match".into(),
                key: now2_ms.clone().into(),
                value: Some(ip127.clone().into()),
                expiry: valid,
            },
            Entry {
                tree: "time-match".into(),
                key: now3_ms.clone().into(),
                value: Some(ip127.clone().into()),
                expiry: valid,
            },
            Entry {
                tree: "time-match".into(),
                key: now2_ms.clone().into(),
                value: Some(ip127.clone().into()),
                expiry: valid,
            },
            Entry {
                tree: "match-timeset".into(),
                key: ip127.clone().into(),
                value: Some([Value::String(now_ms.into())].into()),
                expiry: valid,
            },
            Entry {
                tree: "match-timeset".into(),
                key: ip1.clone().into(),
                value: Some([Value::String(now2_ms.clone().into())].into()),
                expiry: valid,
            },
            Entry {
                tree: "match-timeset".into(),
                key: ip1.clone().into(),
                value: Some([Value::String(now2_ms.clone().into()), now3_ms.into()].into()),
                expiry: valid,
            },
        ];

        let dir = TempDir::new().unwrap();
        let dir_path = dir.path();
        let db_path = dir_path.join(DB_NAME);

        let mut write_db = WriteDB::new(File::create(&db_path).await.unwrap());
        for entry in entries {
            write_db.write_entry(&entry).await.unwrap();
        }
        write_db.close().await.unwrap();
        drop(write_db);

        let mut database = Database::from_dir(&dir_path).await.unwrap();

        let time_match = database
            .open_tree(
                "time-match".into(),
                TimeDelta::seconds(2),
                |(key, value)| Ok((to_time(&key)?, to_match(&value)?)),
            )
            .unwrap();
        assert_eq!(
            time_match.tree,
            BTreeMap::from([
                (now, ip127.clone()),
                (now2, ip127.clone()),
                (now3, ip127.clone())
            ])
        );

        let match_timeset = database
            .open_tree(
                "match-timeset".into(),
                TimeDelta::hours(2),
                |(key, value)| Ok((to_match(&key)?, to_timeset(&value)?)),
            )
            .unwrap();
        assert_eq!(
            match_timeset.tree,
            BTreeMap::from([
                (ip127.clone(), BTreeSet::from([now])),
                (ip1.clone(), BTreeSet::from([now2, now3])),
            ])
        );

        let unknown_tree = database
            .open_tree(
                "unknown_tree".into(),
                TimeDelta::hours(2),
                |(key, value)| Ok((to_match(&key)?, to_timeset(&value)?)),
            )
            .unwrap();
        assert_eq!(unknown_tree.tree, BTreeMap::default());
    }
}
