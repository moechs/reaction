use std::collections::{BTreeMap, HashMap};

use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    fs::{rename, File},
    io::{AsyncBufReadExt, BufReader, BufWriter},
};

use crate::concepts::Time;

const DB_TREE_ID: u32 = 0;

pub struct Database {
    file: BufWriter<File>,
    // TODO file handle
    // TODO channel::receive
}

#[derive(Serialize, Deserialize)]
pub struct Entry {
    #[serde(rename = "t")]
    pub tree: u32,
    #[serde(rename = "k")]
    pub key: Value,
    #[serde(rename = "v")]
    pub value: Option<Value>,
    #[serde(rename = "e")]
    pub expiry: Time,
}

impl Database {
    pub async fn open(path: &str) -> Result<Database, std::io::Error> {
        let file = match File::open(path).await {
            Ok(db) => db,
            Err(err) => match err.kind() {
                std::io::ErrorKind::NotFound => File::create(path).await?,
                _ => return Err(err),
            },
        };
        Ok(Database {
            file: BufWriter::new(file),
        })
    }

    async fn read_existing(
        file: File,
        path: &str,
        new_path: &str,
    ) -> tokio::io::Result<(BufWriter<File>, HashMap<String, HashMap<Value, Value>>)> {
        let mut read_db = ReadDB::new(file);
        let new_file = BufWriter::new(File::open(new_path).await?);

        let mut data_maps = HashMap::new();
        let mut map_ids_to_names = HashMap::new();

        let now = Local::now();

        loop {
            match read_db.next().await {
                Ok(None) => break,
                // Can't recover io::Error here
                Err(ReadError::IO(err)) => return Err(err),
                Err(ReadError::Serde(err)) => todo!("log {err}"),
                Ok(Some(entry)) => {
                    // TODO if metadata add new tree
                    // else
                    // if expired, drop
                    // else add entry in tree and in new db
                    if entry.expiry < now {
                        continue;
                    }
                    if entry.tree == DB_TREE_ID {
                        map_ids_to_names.insert(
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
                        let map: &mut HashMap<Value, Value> =
                            data_maps.entry(entry.tree as u64).or_default();
                        match entry.value {
                            Some(value) => map.insert(entry.key, value),
                            None => map.remove(&entry.key),
                        };
                    }
                }
            }
        }

        let mut return_maps = HashMap::new();
        for (k, v) in data_maps.into_iter() {
            match map_ids_to_names.get(&k) {
                Some(k) => match return_maps.insert(k.to_string(), v) {
                    Some(_) => panic!("multiple database trees had key no {k}"),
                    None => (),
                },
                None => panic!("database did not found name for tree no {k}"),
            }
        }

        rename(new_path, path).await?;
        Ok((new_file, return_maps))
    }
}

enum ReadError {
    IO(std::io::Error),
    Serde(String),
}

struct ReadDB {
    file: BufReader<File>,
    names: BTreeMap<usize, String>,
    buffer: String,
}

impl ReadDB {
    fn new(file: File) -> Self {
        ReadDB {
            file: BufReader::new(file),
            names: BTreeMap::default(),
            buffer: String::default(),
        }
    }
}

impl ReadDB {
    async fn next(&mut self) -> Result<Option<Entry>, ReadError> {
        self.buffer.clear();
        match self.file.read_line(&mut self.buffer).await {
            Ok(0) => return Ok(None),
            Ok(_) => (),
            Err(err) => return Err(ReadError::IO(err)),
        };
        match serde_json::from_str(&self.buffer) {
            Ok(entry) => Ok(Some(entry)),
            Err(err) => Err(ReadError::Serde(err.to_string())),
        }
    }
}
