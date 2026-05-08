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

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::{
    fs::{File, rename},
    sync::{mpsc, oneshot},
    time::{MissedTickBehavior, interval},
};
use tokio_util::{sync::CancellationToken, task::task_tracker::TaskTrackerToken};

// Database

use raw::{ReadDB, WriteDB};
use time::{Time, now};
use tracing::info;

pub mod helpers;
mod raw;
pub mod time;

/// Any order the Database can receive
enum Order {
    Log(Entry),
    OpenTree(OpenTree),
    Rotate,
}

/// Entry sent from [`Tree`] to [`Database`]
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    pub tree: String,
    pub key: Value,
    pub value: Option<Value>,
    pub expiry: Time,
}

/// Order to receive a tree from previous Database
pub struct OpenTree {
    name: String,
    resp: oneshot::Sender<Option<LoadedTree>>,
}

type LoadedTree = HashMap<Value, Value>;
pub type LoadedDB = HashMap<String, LoadedTree>;

const DB_NAME: &str = "reaction.db";
const DB_NEW_NAME: &str = "reaction.new.db";

fn path_of(state_directory: &Path, name: &str) -> PathBuf {
    if state_directory.as_os_str().is_empty() {
        name.into()
    } else {
        PathBuf::from(state_directory).join(name)
    }
}

pub type DatabaseErrorReceiver = oneshot::Receiver<Result<(), String>>;

/// Public-facing API for a treedb Database
pub struct Database {
    entry_tx: Option<mpsc::Sender<Order>>,
    pub error_rx: DatabaseErrorReceiver,
}

impl Database {
    /// Open a new Database, whom task will start in the background.
    /// You'll have to:
    /// - drop all [`Tree`]s,
    /// - call [`Self::quit`],
    ///
    /// to have the Database properly quit.
    ///
    /// You can wait for [`Self::quit`] returned channel to know how it went.
    pub async fn open(
        path_directory: &Path,
        cancellation_token: CancellationToken,
        task_tracker_token: TaskTrackerToken,
    ) -> Result<Database, IoError> {
        let (manager, entry_tx) = DatabaseManager::open(path_directory).await?;
        let error_rx = manager.manager(cancellation_token, task_tracker_token);
        Ok(Self {
            entry_tx: Some(entry_tx),
            error_rx,
        })
    }

    /// Get an independent struct that permits to rotate the database
    pub fn get_rotate(&self) -> Result<DatabaseRotate, String> {
        Ok(DatabaseRotate {
            entry_tx: self.entry_tx.clone().ok_or("Database is closing")?,
        })
    }

    /// Permit to close DB's channel.
    /// The DB closes either when this function is called, or when the [`CancellationToken`] is cancelled.
    pub fn quit(self) -> DatabaseErrorReceiver {
        self.error_rx
    }
}

#[derive(Clone)]
pub struct DatabaseRotate {
    entry_tx: mpsc::Sender<Order>,
}
impl DatabaseRotate {
    /// Rotate the database.
    /// During the rotation process, the database stops processing new entries.
    /// It will read the entire on-disk database, discarding timed-out entries and writing new entries to a new file.
    /// Then the new file will atomically replace the old one, and the database will start processing entries again.
    pub async fn rotate(&mut self) -> Result<(), String> {
        self.entry_tx
            .send(Order::Rotate)
            .await
            .map_err(|_| "Database did not answer".into())
    }
}

/// A [`Database`] logs all write operations on [`Tree`]s in a single file.
/// Logs are written asynchronously, so the write operations in RAM will block only when the
/// underlying channel is full.
struct DatabaseManager {
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
    entry_rx: mpsc::Receiver<Order>,
    /// The interval at which the database must be flushed to kernel
    flush_every: Duration,
}

impl DatabaseManager {
    pub async fn open(
        path_directory: &Path,
    ) -> Result<(DatabaseManager, mpsc::Sender<Order>), IoError> {
        let path = path_of(path_directory, DB_NAME);
        let new_path = path_of(path_directory, DB_NEW_NAME);

        let (write_db, loaded_db) = rotate_db(&path, &new_path, true).await?;

        let (entry_tx, entry_rx) = mpsc::channel(1000);

        Ok((
            DatabaseManager {
                write_db,
                loaded_db,
                path,
                new_path,
                entry_rx,
                flush_every: Duration::from_secs(2),
            },
            entry_tx,
        ))
    }

    pub fn manager(
        mut self,
        cancellation_token: CancellationToken,
        _task_tracker_token: TaskTrackerToken,
    ) -> oneshot::Receiver<Result<(), String>> {
        let (error_tx, error_rx) = oneshot::channel();
        tokio::spawn(async move {
            let mut interval = interval(self.flush_every);
            // If we missed a tick, it will tick immediately, then wait
            // flush_every for the next tick, resulting in a relaxed interval.
            // Hoping this will smooth IO pressure when under heavy load.
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

            let mut status = loop {
                tokio::select! {
                    order = self.entry_rx.recv() => {
                        if let Err(err) = self.handle_order(order).await {
                            cancellation_token.cancel();
                            break err;
                        }
                    }
                    _ = interval.tick() => {
                        if let Err(err) = self.flush().await {
                            cancellation_token.cancel();
                            break Some(err);
                        }
                    }
                    _ = cancellation_token.cancelled() => break None
                };
            };

            // Finish consuming received entries when shutdown asked
            if status.is_none() {
                loop {
                    let order = self.entry_rx.recv().await;
                    if let Err(err) = self.handle_order(order).await {
                        status = err;
                        break;
                    }
                }
            }

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

    /// Executes an order. Returns:
    /// - Err(Some) if there was an error,
    /// - Err(None) is channel is closed,
    /// - Ok(()) in general case.
    async fn handle_order(&mut self, order: Option<Order>) -> Result<(), Option<String>> {
        match order {
            Some(Order::Log(entry)) => self.handle_entry(entry).await.map_err(Option::Some),
            Some(Order::OpenTree(open_tree)) => {
                self.handle_open_tree(open_tree);
                Ok(())
            }
            Some(Order::Rotate) => self
                .rotate_db()
                .await
                .map_err(|err| Option::Some(format!("while rotating database: {err}"))),
            None => Err(None),
        }
    }

    /// Write a received entry.
    async fn handle_entry(&mut self, entry: Entry) -> Result<(), String> {
        match self.write_db.write_entry(&entry).await {
            Ok(_) => Ok(()),
            Err(err) => Err(format!("while writing entry to database: {err}")),
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
        info!("Starting rotation...");
        self.write_db.close().await?;
        let (write_db, _) = rotate_db(&self.path, &self.new_path, false).await?;
        self.write_db = write_db;
        info!("Finished rotation.");
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
                return Ok((WriteDB::new(File::create(path).await?), HashMap::default()));
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
    entry_timeout: Duration,
    /// The inner BTreeMap
    tree: BTreeMap<K, V>,
    /// The sender that permits to asynchronously send write operations to database
    tx: mpsc::Sender<Order>,
}

impl Database {
    /// Creates a new Tree with the given name and entry timeout.
    /// Takes a closure (or regular function) that converts (Value, Value) JSON entries
    /// into (K, V) typed entries.
    /// Helpers for this closure can be found in the [`helpers`] module.
    pub async fn open_tree<K: KeyType, V: ValueType, F>(
        &mut self,
        name: String,
        entry_timeout: Duration,
        map_f: F,
    ) -> Result<Tree<K, V>, String>
    where
        F: Fn((Value, Value)) -> Result<(K, V), String>,
    {
        // Request the tree
        let (tx, rx) = oneshot::channel();
        let entry_tx = match self.entry_tx.clone() {
            None => return Err("Database is closing".to_string()),
            Some(entry_tx) => {
                entry_tx
                    .send(Order::OpenTree(OpenTree {
                        name: name.clone(),
                        resp: tx,
                    }))
                    .await
                    .map_err(|_| "Database did not answer")?;
                // Get a clone of the channel sender
                entry_tx.clone()
            }
        };
        // Load the tree from its JSON
        let tree = if let Some(json_tree) = rx.await.map_err(|_| "Database did not respond")? {
            json_tree
                .into_iter()
                .map(map_f)
                .collect::<Result<BTreeMap<K, V>, String>>()?
        } else {
            BTreeMap::default()
        };
        Ok(Tree {
            id: name,
            entry_timeout,
            tree,
            tx: entry_tx,
        })
    }
}

impl DatabaseManager {
    /// Creates a new Tree with the given name and entry timeout.
    /// Takes a closure (or regular function) that converts (Value, Value) JSON entries
    /// into (K, V) typed entries.
    /// Helpers for this closure can be found in the [`helpers`] module.
    pub fn handle_open_tree(&mut self, open_tree: OpenTree) {
        let _ = open_tree.resp.send(self.loaded_db.remove(&open_tree.name));
    }

    // TODO keep only tree names, and use it for next db rotation to remove associated entries
    // Drops Trees that have not been loaded already
    // pub fn drop_trees(&mut self) {
    //     self.loaded_db = HashMap::default();
    // }
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
    async fn log(&mut self, k: &K, v: Option<&V>) {
        let now = now();
        let e = Entry {
            tree: self.id.clone(),
            key: serde_json::to_value(k).expect("could not serialize key"),
            value: v.map(|v| serde_json::to_value(v).expect("could not serialize value")),
            expiry: now + self.entry_timeout,
        };
        let tx = self.tx.clone();
        // FIXME what if send fails?
        let _ = tx.send(Order::Log(e)).await;
    }

    /// Asynchronously persisted version of [`BTreeMap::insert`]
    pub async fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.log(&key, Some(&value)).await;
        self.tree.insert(key, value)
    }

    /// Asynchronously persisted version of [`BTreeMap::pop_first`]
    pub async fn pop_first(&mut self) -> Option<(K, V)> {
        match self.tree.pop_first() {
            Some((key, value)) => {
                self.log(&key, None).await;
                Some((key, value))
            }
            None => None,
        }
    }

    /// Asynchronously persisted version of [`BTreeMap::pop_last`]
    pub async fn pop_last(&mut self) -> Option<(K, V)> {
        match self.tree.pop_last() {
            Some((key, value)) => {
                self.log(&key, None).await;
                Some((key, value))
            }
            None => None,
        }
    }

    /// Asynchronously persisted version of [`BTreeMap::remove`]
    pub async fn remove(&mut self, key: &K) -> Option<V> {
        self.log(key, None).await;
        self.tree.remove(key)
    }

    /// Updates an item and returns the previous value.
    /// Returning None removes the item if it existed before.
    /// Asynchronously persisted.
    /// *API design borrowed from [`fjall::WriteTransaction::fetch_update`].*
    pub async fn fetch_update<F: FnMut(Option<V>) -> Option<V>>(
        &mut self,
        key: K,
        mut f: F,
    ) -> Option<V> {
        let old_value = self.get(&key).map(|v| v.to_owned());
        let new_value = f(old_value);
        self.log(&key, new_value.as_ref()).await;
        if let Some(new_value) = new_value {
            self.tree.insert(key, new_value)
        } else {
            self.tree.remove(&key)
        }
    }

    #[cfg(any(test, feature = "test"))]
    pub fn tree(&self) -> &BTreeMap<K, V> {
        &self.tree
    }
}

#[cfg(any(test, feature = "test"))]
impl DatabaseManager {
    pub fn set_loaded_db(&mut self, loaded_db: LoadedDB) {
        self.loaded_db = loaded_db;
    }
}

#[cfg(any(test, feature = "test"))]
impl Database {
    pub async fn from_dir(dir_path: &Path, loaded_db: Option<LoadedDB>) -> Result<Self, IoError> {
        use tokio_util::task::TaskTracker;

        let (mut manager, entry_tx) = DatabaseManager::open(dir_path).await?;
        if let Some(loaded_db) = loaded_db {
            manager.set_loaded_db(loaded_db)
        }
        let error_rx = manager.manager(CancellationToken::new(), TaskTracker::new().token());
        Ok(Self {
            entry_tx: Some(entry_tx),
            error_rx,
        })
    }
}

#[cfg(test)]
mod tests {

    use std::{
        collections::{BTreeMap, BTreeSet, HashMap},
        time::Duration,
    };

    use serde_json::Value;
    use tempfile::{NamedTempFile, TempDir};
    use tokio::fs::File;

    use super::{DB_NAME, Database, Entry, Time, helpers::*, now, raw::WriteDB, rotate_db};

    #[tokio::test]
    async fn test_rotate_db() {
        let now = now();

        let expired = now - Time::from_secs(2);
        let valid = now + Time::from_secs(2);

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
        let now = now();

        let now2 = now + Time::from_millis(2);
        let now3 = now + Time::from_millis(3);

        // let now_ms = now.as_nanos().to_string();
        // let now2_ms = now2.as_nanos().to_string();
        // let now3_ms = now3.as_nanos().to_string();

        let valid = now + Time::from_secs(2);

        let ip127 = vec!["127.0.0.1".to_string()];
        let ip1 = vec!["1.1.1.1".to_string()];

        let entries = [
            Entry {
                tree: "time-match".into(),
                key: now.as_nanos().to_string().into(),
                value: Some(ip127.clone().into()),
                expiry: valid,
            },
            Entry {
                tree: "time-match".into(),
                key: now2.as_nanos().to_string().into(),
                value: Some(ip127.clone().into()),
                expiry: valid,
            },
            Entry {
                tree: "time-match".into(),
                key: now3.as_nanos().to_string().into(),
                value: Some(ip127.clone().into()),
                expiry: valid,
            },
            Entry {
                tree: "time-match".into(),
                key: now2.as_nanos().to_string().into(),
                value: Some(ip127.clone().into()),
                expiry: valid,
            },
            Entry {
                tree: "match-timeset".into(),
                key: ip127.clone().into(),
                value: Some([Value::String(now.as_nanos().to_string())].into()),
                expiry: valid,
            },
            Entry {
                tree: "match-timeset".into(),
                key: ip1.clone().into(),
                value: Some([Value::String(now2.as_nanos().to_string())].into()),
                expiry: valid,
            },
            Entry {
                tree: "match-timeset".into(),
                key: ip1.clone().into(),
                value: Some(
                    [
                        Value::String(now2.as_nanos().to_string()),
                        Value::String(now3.as_nanos().to_string()),
                    ]
                    .into(),
                ),
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

        let mut database = Database::from_dir(dir_path, None).await.unwrap();

        let time_match = database
            .open_tree(
                "time-match".into(),
                Duration::from_secs(2),
                |(key, value)| Ok((to_time(&key)?, to_match(&value)?)),
            )
            .await
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
                Duration::from_hours(2),
                |(key, value)| Ok((to_match(&key)?, to_timeset(&value)?)),
            )
            .await
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
                Duration::from_hours(2),
                |(key, value)| Ok((to_match(&key)?, to_timeset(&value)?)),
            )
            .await
            .unwrap();
        assert_eq!(unknown_tree.tree, BTreeMap::default());
    }
}
