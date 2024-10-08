use std::{
    collections::{BTreeMap, HashMap},
    fmt::Debug,
    fs,
    process::exit,
};

use chrono::{Local, TimeDelta};
use log::{debug, error, info, warn};
use thiserror::Error;
use tokio::{sync::mpsc, task};

use super::MatchManagerInput;
use crate::concepts::{ActionFilter, Config, Filter, LogEntry, Match, Time};

mod lowlevel;
mod tests;

use lowlevel::{ReadDB, WriteDB};

const LOG_DB_NAME: &str = "./reaction-matches.db";
const LOG_DB_NEW_NAME: &str = "./reaction-matches.new.db";
const FLUSH_DB_NAME: &str = "./reaction-flushes.db";

const MAX_WRITES: u32 = 500_000;

#[derive(Error, Debug)]
pub enum DBError {
    #[error("invalid filter: {0}")]
    InvalidFilterError(String),
    #[error("decode error: {0}")]
    BincodeError(#[from] bincode::Error),
    #[error("io error: {0}")]
    IOError(#[from] tokio::io::Error),
    #[error("{0}")]
    Error(String),
}

#[derive(Clone)]
pub enum DatabaseManagerInput {
    Log(LogEntry),
    Flush(LogEntry),
}

// Just discovering macros, let me be useless
macro_rules! write_or_die {
    ($db:expr, $entry:expr) => {
        if let Err(err) = $db.write($entry).await {
            error!("Could not write to DB: {}", err);
            exit(1);
        }
    };
}
macro_rules! flush_or_die {
    ($db:expr) => {
        if let Err(err) = $db.flush().await {
            error!("Could not flush DB: {}", err);
            exit(1);
        }
    };
}

/// First rotates the database, then spawns the database thread
pub async fn database_manager(
    config: &'static Config,
    mut log_rx: mpsc::Receiver<DatabaseManagerInput>,
    matches_tx: mpsc::Sender<MatchManagerInput>,
) -> task::JoinHandle<()> {
    let (mut log_db, mut flush_db) = match rotate_db(config, Some(matches_tx)).await {
        Ok(dbs) => dbs,
        Err(err) => {
            error!("while rotating databases on start: {}", err);
            exit(1);
        }
    };

    task::spawn(async move {
        let mut cpt = 0;
        while let Some(order) = log_rx.recv().await {
            match order {
                DatabaseManagerInput::Flush(entry) => write_or_die!(flush_db, entry),
                DatabaseManagerInput::Log(entry) => {
                    write_or_die!(log_db, entry);
                    cpt += 1;
                    if cpt == MAX_WRITES {
                        cpt = 0;
                        flush_or_die!(log_db);
                        flush_or_die!(flush_db);
                        drop(log_db);
                        drop(flush_db);
                        (log_db, flush_db) = match rotate_db(config, None).await {
                            Ok(dbs) => dbs,
                            Err(err) => {
                                error!(
                                    "while rotating databases after {} writes: {}",
                                    MAX_WRITES, err
                                );
                                exit(1);
                            }
                        };
                    }
                }
            };
        }
        flush_or_die!(log_db);
        flush_or_die!(flush_db);
    })
}

async fn rotate_db(
    config: &'static Config,
    matches_tx: Option<mpsc::Sender<MatchManagerInput>>,
) -> Result<(WriteDB, WriteDB), DBError> {
    info!("Rotating database...");
    let res = _rotate_db(config, &matches_tx).await;

    if let Some(tx) = matches_tx {
        #[allow(clippy::unwrap_used)] // propagating panics is ok
        tx.send(MatchManagerInput::EndOfStartup).await.unwrap();
    }

    info!("Rotated database");
    res
}

async fn _rotate_db(
    config: &'static Config,
    matches_tx: &Option<mpsc::Sender<MatchManagerInput>>,
) -> Result<(WriteDB, WriteDB), DBError> {
    // TODO asyncify this
    let mut log_read_db = match ReadDB::open(LOG_DB_NAME, config).await? {
        Some(db) => db,
        None => {
            return Ok(tokio::join!(
                WriteDB::create(LOG_DB_NAME, config),
                WriteDB::create(FLUSH_DB_NAME, config),
            ));
        }
    };
    let mut flush_read_db = match ReadDB::open(FLUSH_DB_NAME, config).await? {
        Some(db) => db,
        None => {
            warn!(
                "Strange! Found a {} but no {}, opening /dev/null instead",
                LOG_DB_NAME, FLUSH_DB_NAME
            );
            match ReadDB::open("/dev/null", config).await? {
                Some(db) => db,
                None => {
                    return Err(DBError::Error("/dev/null is not accessible".into()));
                }
            }
        }
    };

    let mut log_write_db = WriteDB::create(LOG_DB_NEW_NAME, config).await;

    __rotate_db(
        matches_tx,
        &mut log_read_db,
        &mut flush_read_db,
        &mut log_write_db,
    )
    .await;

    drop(log_read_db);
    drop(flush_read_db);

    if let Err(err) = fs::rename(LOG_DB_NEW_NAME, LOG_DB_NAME) {
        return Err(DBError::Error(format!(
            "Failed to replace old DB with new one: {}",
            err
        )));
    }

    if let Err(err) = fs::remove_file(FLUSH_DB_NAME) {
        return Err(DBError::Error(format!("Failed to delete old DB: {}", err)));
    }

    let flush_write_db = WriteDB::create(FLUSH_DB_NAME, config).await;
    Ok((log_write_db, flush_write_db))
}

async fn __rotate_db(
    matches_tx: &Option<mpsc::Sender<MatchManagerInput>>,
    log_read_db: &mut ReadDB,
    flush_read_db: &mut ReadDB,
    log_write_db: &mut WriteDB,
) {
    let mut discarded_errors: HashMap<String, usize> = HashMap::new();

    let mut millisecond_disambiguation_counter: u32 = 0;

    // Read flushes
    let mut flushes: BTreeMap<&'static Filter, BTreeMap<Match, Time>> = BTreeMap::new();
    while let Some(flush_entry) = flush_read_db.next().await {
        match flush_entry {
            Ok(entry) => {
                let matches_map = flushes.entry(entry.f).or_default();
                matches_map.insert(entry.m, entry.t);
            }
            Err(err) => {
                *discarded_errors.entry(err.to_string()).or_insert(0) += 1;
            }
        }
    }
    let last_global_flush = flushes.get(&Filter::default());

    let now = Local::now();

    // Read matches
    while let Some(log_entry) = log_read_db.next().await {
        match log_entry {
            Ok(mut entry) => {
                // Check if number of patterns is in sync
                if entry.m.len() != entry.f.patterns().len() {
                    continue;
                }

                // Check if hasn't been flushed
                if let Some(map) = last_global_flush {
                    if let Some(time) = map.get(&entry.m) {
                        if time > &entry.t {
                            debug!("DB ignoring global match: {:?}", entry.m);
                            continue;
                        }
                    }
                }
                if let Some(map) = flushes.get(&entry.f) {
                    if let Some(time) = map.get(&entry.m) {
                        if time > &entry.t {
                            debug!("DB ignoring local match: {:?}", entry.m);
                            continue;
                        }
                    }
                }

                // Store match & store in db
                if !Filter::is_outdated(&entry, &now) {
                    #[allow(clippy::unwrap_used)] // 0 second is obviously less than i64::MAX
                    {
                        // We loose subsecond precision when storing times, so we add those fake
                        // milliseconds to make sure each time is unique
                        entry.t += TimeDelta::new(0, millisecond_disambiguation_counter).unwrap();
                        millisecond_disambiguation_counter += 1;
                    }

                    if let Some(tx) = matches_tx {
                        debug!("DB sending match from DB: {:?}", entry.m);
                        #[allow(clippy::unwrap_used)] // propagating panics is ok
                        tx.send(MatchManagerInput::Match(entry.clone().into()))
                            .await
                            .unwrap();
                    }

                    write_or_die!(log_write_db, entry);
                }
            }
            Err(err) => {
                *discarded_errors.entry(err.to_string()).or_insert(0) += 1;
            }
        }
    }

    // Warn about errors
    discarded_errors
        .iter()
        .filter(|(_, &count)| count > 0)
        .for_each(|(name, count)| {
            warn!(
                "This problem was found {} times while rotating the database: {}",
                count, name
            )
        });
}
