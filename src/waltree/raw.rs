use std::{collections::HashMap, io::Error as IoError};

use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::{
    fs::{rename, File},
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
};

use crate::concepts::Time;

use super::env::{DatabaseError, Entry};

const DB_TREE_ID: u64 = 0;
const DB_TREE_NAME: &str = "tree_names";

#[derive(Debug, Error)]
enum SerdeOrIoError {
    #[error("{0}")]
    IO(#[from] IoError),
    #[error("{0}")]
    Serde(#[from] serde_json::Error),
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
    pub expiry: &'a Time,
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
    pub expiry: Time,
}

/// Permits to write entries in a database.
/// Entries are written as plain JSONL.
pub struct WriteDB {
    file: BufWriter<File>,
    names: HashMap<String, u64>,
    next_id: u64,
    buffer: String,
}

impl WriteDB {
    /// Creates a wrapper around a file opened in write mode,
    /// To store entries in it.
    pub fn new(file: File) -> Self {
        Self {
            file: BufWriter::new(file),
            names: HashMap::from([(DB_TREE_NAME.into(), DB_TREE_ID)]),
            next_id: 1,
            buffer: String::default(),
        }
    }

    /// Writes an entry in the database
    pub async fn write_entry(&mut self, entry: &Entry) -> Result<(), SerdeOrIoError> {
        let tree_id = match self.names.get(&entry.tree) {
            Some(id) => *id,
            None => {
                let id = self.next_id;
                self.names.insert(entry.tree.clone(), id);
                self.next_id += 1;
                id
            }
        };
        let raw_entry = WriteEntry {
            tree: tree_id,
            key: &entry.key,
            value: &entry.value,
            expiry: &entry.expiry,
        };
        // FIXME reuse the buffer instead of this short-lived allocation
        let mut json = serde_json::to_string(&raw_entry)?;
        json.push('\n');
        self.file.write(json.as_bytes()).await?;
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<(), IoError> {
        self.file.flush().await
    }

    pub async fn close(&mut self) ->  Result<(), IoError> {
        self.file.shutdown().await
    }
}

/// Permits to read entries from a database.
/// The database is plain JSONL.
struct ReadDB {
    file: BufReader<File>,
    names: HashMap<u64, String>,
    buffer: String,
}

impl ReadDB {
    fn new(file: File) -> Self {
        ReadDB {
            file: BufReader::new(file),
            names: HashMap::default(),
            buffer: String::default(),
        }
    }

    async fn read(
        &mut self,
        path: &str,
        new_path: &str,
    ) -> tokio::io::Result<(WriteDB, HashMap<String, HashMap<Value, Value>>)> {
        let mut write_db = WriteDB::new(File::create(new_path).await?);

        let mut data_maps = HashMap::new();

        let now = Local::now();

        loop {
            match self.next().await {
                // EOF
                Ok(None) => break,
                // Can't recover io::Error here
                Err(DatabaseError::IO(err)) => return Err(err),
                // Just skip malformed entries
                Err(DatabaseError::Serde(err)) => todo!("log {err}"),
                Err(DatabaseError::MissingKeyId(id)) => todo!("log {id}"),
                // Ok, we got an entry
                Ok(Some(entry)) => {
                    // Skip expired entries
                    if entry.expiry < now {
                        continue;
                    }
                    if entry.tree == DB_TREE_NAME {
                        self.names.insert(
                            entry
                                .key
                                .as_u64()
                                .expect("database reserved entry doesn't have an int as value")
                                .to_owned(),
                            entry
                                .key
                                .as_str()
                                .expect("database reserved entry doesn't have a string as key")
                                .to_owned(),
                        );
                    } else {
                        // Add back in new DB
                        match write_db.write_entry(&entry).await {
                            Ok(_) => (),
                            Err(err) => match err {
                                SerdeOrIoError::IO(err) => return Err(err),
                                SerdeOrIoError::Serde(err) => panic!("serde should be able to serialize an entry just deserialized: {err}"),
                            }
                        }
                        // Insert data in RAM
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

        rename(new_path, path).await?;
        Ok((write_db, data_maps))
    }
}

impl ReadDB {
    async fn next(&mut self) -> Result<Option<Entry>, DatabaseError> {
        self.buffer.clear();
        match self.file.read_line(&mut self.buffer).await {
            // EOF
            Ok(0) => Ok(None),
            Ok(_) => match serde_json::from_str::<ReadEntry>(&self.buffer) {
                Ok(entry) => match self.names.get(&entry.tree) {
                    Some(tree) => Ok(Some(Entry {
                        tree: tree.to_string(),
                        key: entry.key,
                        value: entry.value,
                        expiry: entry.expiry,
                    })),
                    None => Err(DatabaseError::MissingKeyId(entry.tree)),
                },
                Err(err) => Err(DatabaseError::Serde(err)),
            },
            Err(err) => Err(DatabaseError::IO(err)),
        }
    }
}
