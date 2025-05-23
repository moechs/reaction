use std::collections::HashMap;
use std::io::Error as IoError;
use std::io::ErrorKind;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::fs::File;

use crate::concepts::Time;

use super::raw::WriteDB;

/// Entry sent from BTreeMaps to the Database
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

pub struct Database {
    file: WriteDB,
    path: String,
    new_path: String,
    // TODO channel::receive
}

impl Database {
    pub async fn open(path: String, new_path: String) -> Result<Database, IoError> {
        let file = match File::open(&path).await {
            Ok(db) => db,
            Err(err) => match err.kind() {
                ErrorKind::NotFound => {
                    return Ok(Database {
                        file: WriteDB::new(File::create(&path).await?),
                        path,
                        new_path,
                    })
                }
                _ => return Err(err),
            },
        };
        Ok(Database {
            file: WriteDB::new(file),
            path,
            new_path,
        })
    }

    // TODO déplacer le code de renommage ici, et rendre ReadDB un peu plus bête

    async fn rotate_db_noload() {
    }

    async fn rotate_db_load() -> LoadedDb {
    }
}
