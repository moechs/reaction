use std::{
    collections::{BTreeMap, HashMap},
    fmt::{Debug, Display},
    fs, io,
    path::PathBuf,
    process::exit,
    thread,
};

use chrono::{Local, TimeDelta};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::concepts::{Config, Filter, Match, Time};

mod lowlevel;
mod tests;

use lowlevel::{ReadDB, WriteDB};

enum DatabaseNames {
    LogDbName,
    LogDbNewName,
    FlushDbName,
}
impl DatabaseNames {
    fn basename(&self) -> &'static str {
        match self {
            DatabaseNames::LogDbName => "reaction-matches.db",
            DatabaseNames::LogDbNewName => "reaction-matches.new.db",
            DatabaseNames::FlushDbName => "reaction-flushes.db",
        }
    }
}
impl Display for DatabaseNames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.basename())
    }
}

impl Config {
    fn path_of(&self, name: DatabaseNames) -> PathBuf {
        if self.state_directory().is_empty() {
            name.basename().into()
        } else {
            PathBuf::from(self.state_directory()).join(name.basename())
        }
    }
}
use DatabaseNames::*;

const MAX_WRITES: u32 = 500_000;

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub m: Match,
    pub f: &'static Filter,
    pub t: Time,
    pub exec: bool,
}

impl PartialEq for LogEntry {
    #[allow(clippy::unwrap_used)] // 1 second is obviously less seconds than i64::MAX
    fn eq(&self, other: &Self) -> bool {
        self.exec == other.exec
            && self.m == other.m
            && self.f == other.f
            // We loose subsecond precision while encoding LogEntry
            && (self.t - other.t) < TimeDelta::new(1, 0).unwrap()
    }
}

#[derive(Error, Debug)]
pub enum DBError {
    #[error("invalid filter: {0}")]
    InvalidFilterError(String),
    #[error("decode error: {0}")]
    BincodeError(#[from] bincode::Error),
    #[error("io error: {0}")]
    IOError(#[from] io::Error),
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
        if let Err(err) = $db.write($entry) {
            error!("Could not write to DB: {}", err);
            exit(1);
        }
    };
}
macro_rules! flush_or_die {
    ($db:expr) => {
        if let Err(err) = $db.flush() {
            error!("Could not flush DB: {}", err);
            exit(1);
        }
    };
}

/// First rotates the database, then spawns the database thread
pub fn database_manager(
    config: &'static Config,
    mut log_rx: mpsc::Receiver<DatabaseManagerInput>,
    matches_tx: BTreeMap<&'static Filter, mpsc::Sender<(Match, Time)>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let (mut log_db, mut flush_db) = match rotate_db(config, Some(matches_tx)) {
            Ok(dbs) => dbs,
            Err(err) => {
                error!("while rotating databases on start: {}", err);
                exit(1);
            }
        };

        let mut cpt = 0;
        while let Some(order) = log_rx.blocking_recv() {
            match order {
                DatabaseManagerInput::Flush(entry) => write_or_die!(flush_db, entry),
                DatabaseManagerInput::Log(entry) => {
                    write_or_die!(log_db, entry);
                    cpt += 1;
                    if cpt == MAX_WRITES {
                        info!("Rotating database...");
                        cpt = 0;
                        flush_or_die!(log_db);
                        flush_or_die!(flush_db);
                        drop(log_db);
                        drop(flush_db);
                        (log_db, flush_db) = match rotate_db(config, None) {
                            Ok(dbs) => dbs,
                            Err(err) => {
                                error!(
                                    "while rotating databases after {} writes: {}",
                                    MAX_WRITES, err
                                );
                                exit(1);
                            }
                        };
                        info!("Rotated database");
                    }
                }
            };
        }
        flush_or_die!(log_db);
        flush_or_die!(flush_db);
    })
}

fn rotate_db(
    config: &'static Config,
    matches_tx: Option<BTreeMap<&Filter, mpsc::Sender<(Match, Time)>>>,
) -> Result<(WriteDB, WriteDB), DBError> {
    let mut log_read_db = match ReadDB::open(&config.path_of(LogDbName), config)? {
        Some(db) => db,
        None => {
            return Ok((
                WriteDB::create(&config.path_of(LogDbName), config),
                WriteDB::create(&config.path_of(FlushDbName), config),
            ));
        }
    };
    let mut flush_read_db = ReadDB::open(&config.path_of(FlushDbName), config)?;

    let mut log_write_db = WriteDB::create(&config.path_of(LogDbNewName), config);

    _rotate_db(
        matches_tx,
        &mut log_read_db,
        &mut flush_read_db,
        &mut log_write_db,
    );

    drop(log_read_db);
    drop(flush_read_db);

    if let Err(err) = fs::rename(&config.path_of(LogDbNewName), &config.path_of(LogDbName)) {
        return Err(DBError::Error(format!(
            "Failed to replace old DB with new one: {}",
            err
        )));
    }

    if let Err(err) = fs::remove_file(&config.path_of(FlushDbName)) {
        return Err(DBError::Error(format!("Failed to delete old DB: {}", err)));
    }

    let flush_write_db = WriteDB::create(&config.path_of(FlushDbName), config);
    Ok((log_write_db, flush_write_db))
}

fn _rotate_db(
    matches_tx: Option<BTreeMap<&Filter, mpsc::Sender<(Match, Time)>>>,
    log_read_db: &mut ReadDB,
    flush_read_db: &mut Option<ReadDB>,
    log_write_db: &mut WriteDB,
) {
    let mut discarded_errors: HashMap<String, usize> = HashMap::new();

    let mut millisecond_disambiguation_counter: u32 = 0;

    // Read flushes
    let mut flushes: BTreeMap<&'static Filter, BTreeMap<Match, Time>> = BTreeMap::new();
    if let Some(flush_read_db) = flush_read_db {
        for flush_entry in flush_read_db {
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
    }
    let flushes = flushes;
    debug!("flushes: {:?}", &flushes);

    let last_global_flush = flushes.get(&Filter::default());

    let now = Local::now();

    // Read matches
    for log_entry in log_read_db {
        match log_entry {
            Ok(mut entry) => {
                // Check if number of patterns is in sync
                if entry.m.len() != entry.f.patterns().len() {
                    debug!("DB ignoring entry: nb of patterns in filter not in sync with nb of matches: {:?}", entry);
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

                if entry.t + entry.f.max_time_before_outdated() < now {
                    debug!("DB ignoring outdated match: {:?}", entry);
                    continue;
                }

                // Send match & store in db
                {
                    // We loose subsecond precision when storing times, so we add those fake
                    // milliseconds to make sure each time is unique
                    #![allow(clippy::unwrap_used)] // 0 second is obviously less than i64::MAX
                    entry.t += TimeDelta::new(0, millisecond_disambiguation_counter).unwrap();
                    millisecond_disambiguation_counter += 1;
                }

                if let Some(matches_tx) = &matches_tx {
                    let tx = matches_tx
                        .get(entry.f)
                        .expect("each filter should have an associated channel Sender");
                    debug!("DB sending match from DB: {:?}", entry.m);
                    #[allow(clippy::unwrap_used)] // propagating panics is ok
                    tx.blocking_send((entry.m.clone(), entry.t)).unwrap();
                }

                write_or_die!(log_write_db, entry);
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
