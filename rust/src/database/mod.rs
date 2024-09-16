#![allow(dead_code)]
use std::{
    collections::BTreeMap,
    fmt::Display,
    fs::{self, File},
    process::exit,
    sync::{
        mpsc::{Receiver, Sender},
        Arc,
    },
    thread,
};

use chrono::{DateTime, Local};
use log::{debug, error, info, warn};
use postcard::{from_io, to_io, Error};
use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    filter::FilterName,
    matches::MatchManagerInput,
    messages::{LogEntry, Match, Time},
};

mod tests;

const LOG_DB_NAME: &str = "./reaction-matches.db";
const LOG_DB_NEW_NAME: &str = "./reaction-matches.new.db";
const FLUSH_DB_NAME: &str = "./reaction-flushes.db";

const MAX_WRITES: u32 = 500_000;

// FIXME how to use postcard::from_io with a dynamic buffer?
const HEADER_MAX_SIZE: usize = 1024 * 1024;
const ENTRY_MAX_SIZE: usize = 1024 * 1024;

struct ReadDB {
    f: File,
    h: DatabaseReadHeader,
}

impl ReadDB {
    fn open(path: &str) -> Result<Option<Self>, DBError> {
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(err) => match err.kind() {
                std::io::ErrorKind::NotFound => {
                    warn!(
                        "No DB found at {}. It's ok if this is the first time reaction is running.",
                        path
                    );
                    return Ok(None);
                }
                _ => {
                    return Err(DBError::Error(format!("Could not open database: {}", err)));
                }
            },
        };

        let mut buf: [u8; HEADER_MAX_SIZE] = [0; HEADER_MAX_SIZE];
        let (database_header, _) = from_io((&mut file, &mut buf))?;

        Ok(Some(ReadDB {
            f: file,
            h: database_header,
        }))
    }
}

impl Iterator for ReadDB {
    type Item = Result<LogEntry, DBError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut buf: [u8; ENTRY_MAX_SIZE] = [0; ENTRY_MAX_SIZE];
        let result = from_io::<ComputedLogEntry, _>((&mut self.f, &mut buf));
        match result {
            // FIXME why we got a default item instead of an error or something?
            // How do we really know we reached the end?
            Ok((item, _)) => {
                if item.t == 0 {
                    None
                } else {
                    Some(item.to(&self.h))
                }
            }
            Err(err) => match err {
                Error::DeserializeUnexpectedEnd => None,
                _ => Some(Err(err.into())),
            },
        }
    }
}

struct WriteDB {
    f: File,
    h: DatabaseWriteHeader,
}

impl WriteDB {
    fn create(path: &str, config: &Arc<Config>) -> Self {
        let mut file = match File::create(path) {
            Ok(file) => file,
            Err(err) => {
                error!("Failed to create DB: {}", err);
                exit(1);
            }
        };

        let database_read_header: BTreeMap<usize, FilterName> =
            config.filter_names().into_iter().enumerate().collect();

        if let Err(err) = to_io(&database_read_header, &mut file) {
            error!("Failed to write to DB: {}", err);
            exit(1);
        }

        let database_write_header = database_read_header
            .into_iter()
            .map(|(i, name)| (name, i as u16))
            .collect();

        WriteDB {
            f: file,
            h: database_write_header,
        }
    }

    fn write(&mut self, entry: LogEntry) -> Result<(), DBError> {
        let computed = ComputedLogEntry::from(entry, &self.h)?;
        to_io(&computed, &mut self.f)?;
        Ok(())
    }
}

#[derive(Debug)]
enum DBError {
    InvalidFilterError(String),
    PostcardError(postcard::Error),
    Error(String),
}

impl From<postcard::Error> for DBError {
    fn from(value: postcard::Error) -> Self {
        DBError::PostcardError(value)
    }
}

impl Display for DBError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DBError::InvalidFilterError(e) => write!(f, "invalid filter : {}", e),
            DBError::PostcardError(e) => write!(f, "decode error: {}", e),
            DBError::Error(e) => write!(f, "{}", e),
        }
    }
}

type DatabaseWriteHeader = BTreeMap<FilterName, u16>;
type DatabaseReadHeader = BTreeMap<u16, FilterName>;

#[derive(Serialize, Deserialize)]
struct ComputedLogEntry {
    pub m: Match,
    pub f: u16,
    pub t: i64,
    pub exec: bool,
}

impl ComputedLogEntry {
    fn from(value: LogEntry, header: &DatabaseWriteHeader) -> Result<Self, DBError> {
        match header.get(&value.f) {
            Some(f) => Ok(ComputedLogEntry {
                m: value.m,
                f: *f,
                t: value.t.timestamp(),
                exec: value.exec,
            }),
            None => Err(DBError::InvalidFilterError(value.f.to_string())),
        }
    }
    fn to(self, header: &DatabaseReadHeader) -> Result<LogEntry, DBError> {
        match header.get(&self.f) {
            Some(f) => Ok(LogEntry {
                m: self.m,
                f: f.clone(),
                t: DateTime::from_timestamp(self.t, 0)
                    .unwrap()
                    .with_timezone(&Local),
                exec: self.exec,
            }),
            None => Err(DBError::InvalidFilterError(self.f.to_string())),
        }
    }
}

#[derive(Clone)]
pub enum DatabaseManagerInput {
    Log(LogEntry),
    Flush(LogEntry),
}

/// First rotates the database, then spawns the database thread
pub fn database_manager(
    config: Arc<Config>,
    log_rx: Receiver<DatabaseManagerInput>,
    matches_tx: Sender<MatchManagerInput>,
) -> thread::JoinHandle<()> {
    let (mut log_db, mut flush_db) = match rotate_db(&config, Some(matches_tx)) {
        Ok(dbs) => dbs,
        Err(err) => {
            error!("while rotating databases on start: {}", err);
            exit(1);
        }
    };

    thread::spawn(move || {
        let mut cpt = 0;
        while let Ok(order) = log_rx.recv() {
            match order {
                DatabaseManagerInput::Flush(entry) => flush_db.write(entry).unwrap(),
                DatabaseManagerInput::Log(entry) => {
                    log_db.write(entry).unwrap();
                    cpt += 1;
                    if cpt == MAX_WRITES {
                        info!("Rotating database...");
                        cpt = 0;
                        drop(log_db);
                        drop(flush_db);
                        (log_db, flush_db) = match rotate_db(&config, None) {
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
    })
}

fn rotate_db(
    config: &Arc<Config>,
    matches_tx: Option<Sender<MatchManagerInput>>,
) -> Result<(WriteDB, WriteDB), DBError> {
    let mut log_read_db = match ReadDB::open(LOG_DB_NAME)? {
        Some(db) => db,
        None => {
            return Ok((
                WriteDB::create(LOG_DB_NAME, config),
                WriteDB::create(FLUSH_DB_NAME, config),
            ));
        }
    };
    let mut flush_read_db = match ReadDB::open(FLUSH_DB_NAME)? {
        Some(db) => db,
        None => {
            warn!(
                "Strange! Found a {} but no {}, opening /dev/null instead",
                LOG_DB_NAME, FLUSH_DB_NAME
            );
            match ReadDB::open("/dev/null")? {
                Some(db) => db,
                None => {
                    return Err(DBError::Error("/dev/null is not accessible".into()));
                }
            }
        }
    };

    let mut log_write_db = WriteDB::create(LOG_DB_NEW_NAME, config);

    _rotate_db(
        config,
        matches_tx,
        &mut log_read_db,
        &mut flush_read_db,
        &mut log_write_db,
    );

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

    let flush_write_db = WriteDB::create(FLUSH_DB_NAME, config);
    Ok((log_write_db, flush_write_db))
}

fn _rotate_db(
    config: &Arc<Config>,
    matches_tx: Option<Sender<MatchManagerInput>>,
    log_read_db: &mut ReadDB,
    flush_read_db: &mut ReadDB,
    log_write_db: &mut WriteDB,
) {
    let mut discarded_entries: BTreeMap<FilterName, usize> = BTreeMap::new();
    let mut discarded_count: usize = 0;

    // Read flushes
    let mut flushes: BTreeMap<FilterName, BTreeMap<Match, Time>> = BTreeMap::new();
    for flush_entry in flush_read_db {
        match flush_entry {
            Ok(entry) => {
                if config
                    .get_filter(&entry.f.stream, &entry.f.filter)
                    .is_some()
                {
                    let matches_map = flushes.entry(entry.f).or_default();
                    matches_map.insert(entry.m, entry.t);
                } else {
                    *discarded_entries.entry(entry.f).or_insert(0) += 1;
                }
            }
            Err(err) => {
                debug!("while reading flush db: {}", err);
                discarded_count += 1;
            }
        }
    }
    let global_flush_map = flushes.get(&FilterName {
        stream: "".into(),
        filter: "".into(),
    });

    let now = Local::now();

    // Read matches
    for log_entry in log_read_db {
        match log_entry {
            Ok(entry) => {
                // retrieve related stream & filter
                let filter = match config.get_filter(&entry.f.stream, &entry.f.filter) {
                    Some(filter) => filter,
                    None => {
                        *discarded_entries.entry(entry.f).or_insert(0) += 1;
                        continue;
                    }
                };

                // Check if number of patterns is in sync
                if entry.m.len() != filter.patterns().len() {
                    *discarded_entries.entry(entry.f).or_insert(0) += 1;
                    continue;
                }

                // Check if hasn't been flushed
                if let Some(map) = global_flush_map {
                    if let Some(time) = map.get(&entry.m) {
                        if time > &entry.t {
                            continue;
                        }
                    }
                }
                if let Some(map) = flushes.get(&entry.f) {
                    if let Some(time) = map.get(&entry.m) {
                        if time > &entry.t {
                            continue;
                        }
                    }
                }

                // Store match & store in db
                if (!entry.exec && entry.t + filter.retry_duration().unwrap_or_default() > now)
                    || (entry.exec && entry.t + filter.longuest_action_duration() > now)
                {
                    if let Some(tx) = &matches_tx {
                        tx.send(MatchManagerInput::Match(entry.clone().into()))
                            .unwrap();
                    }

                    if let Err(err) = log_write_db.write(entry) {
                        error!("Could not write to DB: {}", err);
                        exit(1);
                    }
                }
            }
            Err(err) => {
                debug!("while reading log db: {}", err);
                discarded_count += 1;
            }
        }
    }

    if let Some(tx) = matches_tx {
        tx.send(MatchManagerInput::EndOfStartup).unwrap();
    }

    // Warn about errors
    discarded_entries.iter()
        .filter(|(_, &count)| count > 0)
        .for_each(|(name, count)| warn!("{} entries discarded from the databases: stream/filter not found in configuration, or the patterns are not the same: {}", count, name));

    if discarded_count > 0 {
        error!(
            "{} entries discarded from the databases (garbage while decoding)",
            discarded_count
        );
    }
}
