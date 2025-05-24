use std::collections::BTreeMap;
use std::collections::HashMap;
use std::io::Error as IoError;
use std::io::ErrorKind;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Local;
use chrono::TimeDelta;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::sleep;
use tokio::{
    fs::{rename, File},
    sync::mpsc,
};
use tracing::error;

use crate::concepts::Config;
use crate::concepts::Time;

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

pub type LoadedDb = HashMap<String, HashMap<Value, Value>>;

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

/// A [`Database`] logs all write operations on [`Tree`]s in a single file.
/// Logs are written asynchronously, so the write operations in RAM will never block.
pub struct Database {
    /// Inner database
    write_db: WriteDB,
    /// Path for the "normal" database
    path: PathBuf,
    /// Path for the "new" database, when rotating database.
    /// New database atomically replaces the old one when its writing is done.
    new_path: PathBuf,
    /// The receiver on [`Tree`] write operations
    rx: mpsc::Receiver<Entry>,
    /// The sender on [`Tree`] write operations.
    /// Only used to clone new senders for new Trees.
    tx: mpsc::Sender<Entry>,
    /// The interval at which the database must be flushed to kernel
    flush_every: Duration,
    /// The maximum bytes that must be written until the database is rotated
    max_bytes: usize,
    /// Counter to account of the current number of bytes written
    bytes_written: usize,
}

impl Database {
    pub async fn open(config: Config) -> Result<(Database, LoadedDb), IoError> {
        let path = config.path_of(DB_NAME);
        let new_path = config.path_of(DB_NEW_NAME);

        let (write_db, loaded_db) = rotate_db(&path, &new_path, true).await?;

        let (tx, rx) = mpsc::channel(1000);

        Ok((
            Database {
                write_db,
                path,
                new_path,
                rx,
                tx,
                flush_every: Duration::from_secs(2),
                max_bytes: 20 * 1024 * 1024, // 20 MiB
                bytes_written: 0,
            },
            loaded_db,
        ))
    }

    pub async fn task(&mut self) {
        // FIXME this never flushes if entries keep coming, as receiving an entry cancels the sleep
        // replace by tokio::time::Interval::Skip
        loop {
            tokio::select! {
                entry = self.rx.recv() => {
                    if !self.handle_entry(entry).await {
                        break;
                    }
                }
                _ = sleep(self.flush_every) => {
                    if !self.flush().await {
                        break;
                    }
                }
            };
        }

        // Shutdown
        if let Err(err) = self.write_db.close().await {
            error!("while closing database: {err}");
        }
    }

    /// Write a received entry.
    /// Return false if there was an error or if it was the last entry.
    async fn handle_entry(&mut self, entry: Option<Entry>) -> bool {
        match entry {
            Some(entry) => match self.write_db.write_entry(&entry).await {
                Ok(bytes_written) => {
                    self.bytes_written += bytes_written;
                    if self.bytes_written > self.max_bytes {
                        match self.rotate_db().await {
                            Ok(()) => {
                                self.bytes_written = 0;
                                true
                            }
                            Err(err) => {
                                error!("while rotating database: {err}");
                                false
                            }
                        }
                    } else {
                        true
                    }
                }
                Err(err) => {
                    error!("while writing entry to database: {err}");
                    false
                }
            },
            None => false,
        }
    }

    /// Flush inner database.
    /// Return false if there was an error
    async fn flush(&mut self) -> bool {
        match self.write_db.flush().await {
            Ok(()) => true,
            Err(err) => {
                error!("while flushing database: {err}");
                false
            }
        }
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
) -> Result<(WriteDB, LoadedDb), IoError> {
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
    pub fn open_tree<K: KeyType, V: ValueType>(
        &mut self,
        name: String,
        entry_timeout: TimeDelta,
    ) -> Tree<K, V> {
        // TODO load from LoadedDb
        Tree {
            id: name,
            entry_timeout,
            tree: BTreeMap::new(),
            tx: self.tx.clone(),
        }
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
        tokio::spawn(async move { tx.send(e).await });
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
}

#[cfg(test)]
mod tests {

    use std::collections::HashMap;

    use chrono::{Local, TimeDelta};
    use tempfile::NamedTempFile;
    use tokio::fs::File;

    use crate::waltree::{raw::WriteDB, rotate_db, Entry};

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
}
