use std::{collections::HashMap, io::Error as IoError};

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
};
use tracing::error;

use super::env::{Entry, LoadedDb};

const DB_TREE_ID: u64 = 0;
// const DB_TREE_NAME: &str = "tree_names";

#[derive(Debug, Error)]
pub enum SerdeOrIoError {
    #[error("{0}")]
    IO(#[from] IoError),
    #[error("{0}")]
    Serde(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("{0}")]
    IO(#[from] IoError),
    #[error("{0}")]
    Serde(#[from] serde_json::Error),
    #[error("missing key id: {0}")]
    MissingKeyId(u64),
}

/// Entry in custom database format, just to be written in database
#[derive(Serialize)]
struct WriteEntry<'a> {
    #[serde(rename = "t")]
    pub tree: u64,
    #[serde(rename = "k")]
    pub key: &'a Value,
    #[serde(rename = "v")]
    pub value: &'a Option<Value>,
    #[serde(rename = "e")]
    pub expiry: i64,
}

/// Entry in custom database format, just read from database
#[derive(Deserialize)]
struct ReadEntry {
    #[serde(rename = "t")]
    pub tree: u64,
    #[serde(rename = "k")]
    pub key: Value,
    #[serde(rename = "v")]
    pub value: Option<Value>,
    #[serde(rename = "e")]
    pub expiry: i64,
}

/// Permits to write entries in a database.
/// Entries are written as plain JSONL.
pub struct WriteDB {
    file: BufWriter<File>,
    names: HashMap<String, u64>,
    next_id: u64,
    // buffer: String,
}

impl WriteDB {
    /// Creates a wrapper around a file opened in write mode,
    /// To store entries in it.
    pub fn new(file: File) -> Self {
        Self {
            file: BufWriter::new(file),
            // names: HashMap::from([(DB_TREE_NAME.into(), DB_TREE_ID)]),
            names: HashMap::default(),
            next_id: 1,
            // buffer: String::default(),
        }
    }

    /// Writes an entry in the database
    pub async fn write_entry(&mut self, entry: &Entry) -> Result<(), SerdeOrIoError> {
        let tree_id = match self.names.get(&entry.tree) {
            Some(id) => *id,
            // Insert special database entry when the tree is not recorded yet
            None => {
                let id = self.next_id;
                self.next_id += 1;

                self.names.insert(entry.tree.clone(), id);
                self._write_entry(&WriteEntry {
                    tree: DB_TREE_ID,
                    key: &id.into(),
                    value: &Some(entry.tree.clone().into()),
                    // Expiry is not used for special entries
                    expiry: 0,
                })
                .await?;

                id
            }
        };
        self._write_entry(&WriteEntry {
            tree: tree_id,
            key: &entry.key,
            value: &entry.value,
            expiry: entry.expiry.timestamp_millis(),
        })
        .await
    }

    async fn _write_entry(&mut self, raw_entry: &WriteEntry<'_>) -> Result<(), SerdeOrIoError> {
        // FIXME reuse the buffer instead of this short-lived allocation
        let mut json = serde_json::to_string(&raw_entry)?;
        json.push('\n');
        self.file.write(json.as_bytes()).await?;
        Ok(())
    }

    /// Flushes the inner [`tokio::io::BufWriter`]
    pub async fn flush(&mut self) -> Result<(), IoError> {
        self.file.flush().await
    }

    /// Closes the inner [`tokio::io::BufWriter`]
    /// WriteDB should not be used after this point.
    pub async fn close(&mut self) -> Result<(), IoError> {
        self.file.shutdown().await
    }
}

/// Permits to read entries from a database.
/// The database is plain JSONL.
pub struct ReadDB {
    file: BufReader<File>,
    names: HashMap<u64, String>,
    buffer: String,
}

impl ReadDB {
    pub fn new(file: File) -> Self {
        ReadDB {
            file: BufReader::new(file),
            // names: HashMap::from([(DB_TREE_ID, DB_TREE_NAME.into())]),
            names: HashMap::default(),
            buffer: String::default(),
        }
    }

    pub async fn read(
        &mut self,
        write_db: &mut WriteDB,
        load_db: bool,
    ) -> tokio::io::Result<LoadedDb> {
        let mut data_maps = HashMap::new();

        loop {
            match self.next().await {
                // EOF
                Ok(None) => break,
                // Can't recover io::Error here
                Err(DatabaseError::IO(err)) => return Err(err),
                // TODO Just skip malformed entries
                Err(DatabaseError::Serde(err)) => {
                    error!("malformed entry read from database: {err}")
                }
                Err(DatabaseError::MissingKeyId(id)) => {
                    error!("invalid entry read from database: {id}")
                }
                // Ok, we got an entry
                Ok(Some(entry)) => {
                    // Add back in new DB
                    match write_db.write_entry(&entry).await {
                            Ok(_) => (),
                            Err(err) => match err {
                                SerdeOrIoError::IO(err) => return Err(err),
                                SerdeOrIoError::Serde(err) => panic!("serde should be able to serialize an entry just deserialized: {err}"),
                            }
                        }
                    // Insert data in RAM
                    if load_db {
                        let map: &mut HashMap<Value, Value> =
                            data_maps.entry(entry.tree).or_default();
                        match entry.value {
                            Some(value) => map.insert(entry.key, value),
                            None => map.remove(&entry.key),
                        };
                    }
                }
            }
        }

        Ok(data_maps)
    }

    async fn next(&mut self) -> Result<Option<Entry>, DatabaseError> {
        let now = Local::now().timestamp_millis();
        // Loop until we get a non-special value
        let raw_entry = loop {
            self.buffer.clear();
            let raw_entry = match self.file.read_line(&mut self.buffer).await {
                // EOF
                Ok(0) => return Ok(None),
                Ok(_) => match serde_json::from_str::<ReadEntry>(&self.buffer) {
                    Ok(raw_entry) => Ok(raw_entry),
                    Err(err) => Err(DatabaseError::Serde(err)),
                },
                Err(err) => Err(DatabaseError::IO(err)),
            }?;

            if raw_entry.tree == DB_TREE_ID {
                // Insert new tree
                self.names.insert(
                    raw_entry
                        .key
                        .as_u64()
                        .expect("database reserved entry doesn't have an uint as key")
                        .to_owned(),
                    raw_entry
                        .value
                        .expect("database reserved entry doesn't have a value")
                        .as_str()
                        .expect("database reserved entry doesn't have a string as value")
                        .to_owned(),
                );
                // Skip expired entries
            } else if raw_entry.expiry >= now {
                break raw_entry;
            }
        };

        match self.names.get(&raw_entry.tree) {
            Some(tree) => Ok(Some(Entry {
                tree: tree.to_owned(),
                key: raw_entry.key,
                value: raw_entry.value,
                expiry: Local.timestamp_millis_opt(raw_entry.expiry).unwrap(),
            })),
            None => Err(DatabaseError::MissingKeyId(raw_entry.tree)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{Local, TimeDelta, TimeZone};
    use serde_json::Value;
    use tempfile::NamedTempFile;
    use tokio::fs::{read, write, File};

    use crate::waltree::{
        env::Entry,
        raw::{DatabaseError, ReadDB, WriteDB, DB_TREE_ID},
    };

    #[tokio::test]
    async fn write_db_write_entry() {
        let now = Local::now();
        let expired = now - TimeDelta::seconds(2);
        let expired_ts = expired.timestamp_millis();
        // let valid = now + TimeDelta::seconds(2);
        // let valid_ts = valid.timestamp_millis();

        let path = NamedTempFile::new().unwrap().into_temp_path();

        let mut write_db = WriteDB::new(File::create(&path).await.unwrap());

        write_db
            .write_entry(&Entry {
                tree: "yooo".into(),
                key: "key1".into(),
                value: Some("value1".into()),
                expiry: expired,
            })
            .await
            .unwrap();
        write_db.flush().await.unwrap();

        let contents = String::from_utf8(read(path).await.unwrap()).unwrap();

        println!("{}", &contents);

        assert_eq!(
            contents,
            format!("{{\"t\":0,\"k\":1,\"v\":\"yooo\",\"e\":0}}\n{{\"t\":1,\"k\":\"key1\",\"v\":\"value1\",\"e\":{expired_ts}}}\n")
        );
    }

    #[tokio::test]
    async fn read_db_next() {
        let now = Local::now();

        let expired = now - TimeDelta::seconds(2);
        let expired_ts = expired.timestamp_millis();

        let valid = now + TimeDelta::seconds(2);
        let valid_ts = valid.timestamp_millis();
        // Truncate to millisecond precision
        let valid = Local.timestamp_millis_opt(valid_ts).unwrap();

        let path = NamedTempFile::new().unwrap().into_temp_path();

        write(
            &path,
            format!(
                "{{\"t\": {DB_TREE_ID}, \"k\": 1, \"v\": \"test_tree\", \"e\": 0}}
                 {{\"t\": 1, \"k\": \"key1\", \"v\": 1, \"e\": {expired_ts}}}
                 {{\"t\": 1, \"k\": \"key2\", \"v\": 2, \"e\": {valid_ts}}}
                 malformed entry: not json
                 {{\"t\": \"tree cant be string\", \"k\": \"key2\", \"v\": 2, \"e\": {valid_ts}}}
                 {{\"t\": 1, \"k\": \"missing quote, \"v\": 2, \"e\": {valid_ts}}}
                 {{\"t\": 3, \"k\": \"missing key id\", \"v\": 2, \"e\": {valid_ts}}}"
            ),
        )
        .await
        .unwrap();

        let mut read_db = ReadDB::new(File::open(path).await.unwrap());

        assert_eq!(
            read_db.next().await.unwrap(),
            Some(Entry {
                tree: "test_tree".into(),
                key: "key2".into(),
                value: Some(2.into()),
                expiry: valid,
            })
        );

        assert!(match read_db.next().await {
            Err(DatabaseError::Serde(_)) => true,
            _ => false,
        });
        assert!(match read_db.next().await {
            Err(DatabaseError::Serde(_)) => true,
            _ => false,
        });
        assert!(match read_db.next().await {
            Err(DatabaseError::Serde(_)) => true,
            _ => false,
        });
        assert!(match read_db.next().await {
            Err(DatabaseError::MissingKeyId(3)) => true,
            _ => false,
        });
        assert!(read_db.next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn read_db_read() {
        let now = Local::now();

        let expired = now - TimeDelta::seconds(2);
        let expired_ts = expired.timestamp_millis();

        let valid = now + TimeDelta::seconds(2);
        let valid_ts = valid.timestamp_millis();

        let read_path = NamedTempFile::new().unwrap().into_temp_path();
        let write_path = NamedTempFile::new().unwrap().into_temp_path();

        write(
            &read_path,
            format!(
                "{{\"t\": {DB_TREE_ID}, \"k\": 1, \"v\": \"test_tree\", \"e\": 0}}
                 {{\"t\": 1, \"k\": \"key1\", \"v\": 1, \"e\": {expired_ts}}}
                 {{\"t\": 1, \"k\": \"key2\", \"v\": 2, \"e\": {valid_ts}}}
                 malformed entry: not json
                 {{\"t\": \"tree cant be string\", \"k\": \"key2\", \"v\": 2, \"e\": {valid_ts}}}
                 {{\"t\": 1, \"k\": \"missing quote, \"v\": 2, \"e\": {valid_ts}}}
                 {{\"t\": 3, \"k\": \"missing key id\", \"v\": 2, \"e\": {valid_ts}}}"
            ),
        )
        .await
        .unwrap();

        // First test loading in RAM

        let mut read_db = ReadDB::new(File::open(&read_path).await.unwrap());
        let mut write_db = WriteDB::new(File::create(&write_path).await.unwrap());
        let maps = read_db.read(&mut write_db, true).await.unwrap();

        // Test RAM
        assert_eq!(
            maps,
            HashMap::from([(
                "test_tree".into(),
                HashMap::from([("key2".into(), 2.into())])
            )])
        );

        // Test disk
        write_db.close().await.unwrap();
        let contents = String::from_utf8(read(&write_path).await.unwrap()).unwrap();

        assert_eq!(
            contents,
            format!(
                "{{\"t\":{DB_TREE_ID},\"k\":1,\"v\":\"test_tree\",\"e\":0}}\n\
                 {{\"t\":1,\"k\":\"key2\",\"v\":2,\"e\":{valid_ts}}}\n"
            )
        );

        // Second test only rotating on disk

        let mut read_db = ReadDB::new(File::open(&read_path).await.unwrap());
        let mut write_db = WriteDB::new(File::create(&write_path).await.unwrap());
        let maps = read_db.read(&mut write_db, false).await.unwrap();

        // Test RAM
        assert_eq!(maps, HashMap::default());

        // Test disk
        write_db.close().await.unwrap();
        let contents = String::from_utf8(read(write_path).await.unwrap()).unwrap();

        assert_eq!(
            contents,
            format!(
                "{{\"t\":{DB_TREE_ID},\"k\":1,\"v\":\"test_tree\",\"e\":0}}\n\
                 {{\"t\":1,\"k\":\"key2\",\"v\":2,\"e\":{valid_ts}}}\n"
            )
        );
    }

    // write then read 1000 random entries
    #[tokio::test]
    async fn write_then_read_1000() {
        // Generate entries
        let now = Local::now();
        let entries: Vec<_> = (0..1000)
            .map(|i| Entry {
                tree: format!("tree{}", i % 4),
                key: format!("key{}", i % 10).into(),
                value: Some(format!("value{}", i % 10).into()),
                expiry: now + TimeDelta::seconds((i % 4) - 1),
            })
            .collect();

        let remove_entries: Vec<_> = (0..1000)
            .filter(|i| i % 5 == 1)
            .map(|i| Entry {
                tree: format!("tree{}", i % 4),
                key: format!("key{}", i % 10).into(),
                value: None,
                expiry: now + TimeDelta::seconds(i % 4),
            })
            .collect();

        let all_entries: Vec<_> = entries.iter().chain(remove_entries.iter()).collect();

        let kept_entries: HashMap<String, HashMap<Value, Value>> = entries
            .iter()
            .filter(|entry| entry.expiry > now)
            .filter(|entry| {
                remove_entries
                    .iter()
                    .all(|rm_entry| rm_entry.tree != entry.tree || rm_entry.key != entry.key)
            })
            .fold(HashMap::default(), |mut acc, entry| {
                acc.entry(entry.tree.clone()).or_default().insert(
                    entry.key.clone(),
                    entry.value.clone().unwrap(),
                );
                acc
            });

        // Write entries
        let read_path = NamedTempFile::new().unwrap().into_temp_path();
        let write_path = NamedTempFile::new().unwrap().into_temp_path();

        let mut write_db = WriteDB::new(File::create(&read_path).await.unwrap());
        for entry in all_entries {
            write_db.write_entry(entry).await.unwrap();
        }
        write_db.close().await.unwrap();

        // Read entries
        let mut read_db = ReadDB::new(File::open(&read_path).await.unwrap());
        let mut write_db = WriteDB::new(File::create(&write_path).await.unwrap());
        let maps = read_db
            .read(&mut write_db, true)
            .await
            .unwrap()
            .into_iter()
            .filter(|(_, map)| !map.is_empty())
            .collect::<HashMap<_, _>>();

        assert_eq!(maps, kept_entries);
    }
}
