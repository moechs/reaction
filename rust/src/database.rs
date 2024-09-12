#![allow(dead_code)]
use std::{
    collections::BTreeMap,
    fs::{self, File},
    process::exit,
    sync::{mpsc::Receiver, Arc},
};

use chrono::{DateTime, Local};
use log::{debug, error, info, warn};
use postcard::{from_io, to_io, Error};
use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    filter::FilterName,
    messages::{LogEntry, Match, Time},
};

const LOG_DB_NAME: &str = "./reaction-matches.db";
const LOG_DB_NEW_NAME: &str = "./reaction-matches.new.db";
const FLUSH_DB_NAME: &str = "./reaction-flushes.db";

struct ReadDB {
    f: File,
    h: DatabaseReadHeader,
}

impl ReadDB {
    fn open(path: &str) -> Option<Self> {
        let mut file = match File::open(path) {
            Ok(file) => Some(file),
            Err(err) => match err.kind() {
                std::io::ErrorKind::NotFound => {
                    warn!(
                        "No DB found at {}. It's ok if this is the first time reaction is running.",
                        path
                    );
                    None
                }
                _ => {
                    error!("Failed to open DB: {}", err);
                    exit(1);
                }
            },
        }?;

        let mut buf: Vec<u8> = Vec::new();
        let (database_header, _) = from_io((&mut file, &mut buf)).unwrap();

        Some(ReadDB {
            f: file,
            h: database_header,
        })
    }
}

impl Iterator for ReadDB {
    type Item = Result<LogEntry, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut buf: Vec<u8> = Vec::new();
        let result = from_io::<ComputedLogEntry, _>((&mut self.f, &mut buf));
        match result {
            Ok((item, _)) => Some(Ok(item.to(&self.h))),
            Err(err) => match err {
                Error::DeserializeUnexpectedEnd => None,
                _ => Some(Err(err)),
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

        let database_header = config
            .filter_names()
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name, i as u16))
            .collect();

        if let Err(err) = to_io(&database_header, &mut file) {
            error!("Failed to write to DB: {}", err);
            exit(1);
        }

        WriteDB {
            f: file,
            h: database_header,
        }
    }

    fn write(&mut self, entry: LogEntry) {
        let computed = ComputedLogEntry::from(entry, &self.h);
        to_io(&computed, &mut self.f).unwrap();
    }
}

#[derive(Clone)]
pub enum DatabaseInput {
    Log(LogEntry),
    Flush(LogEntry),
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
    fn from(value: LogEntry, header: &DatabaseWriteHeader) -> Self {
        ComputedLogEntry {
            m: value.m,
            f: *header.get(&value.f).unwrap(),
            t: value.t.timestamp(),
            exec: value.exec,
        }
    }
    fn to(self, header: &DatabaseReadHeader) -> LogEntry {
        LogEntry {
            m: self.m,
            f: header.get(&self.f).unwrap().clone(),
            t: DateTime::from_timestamp(self.t, 0)
                .unwrap()
                .with_timezone(&Local),
            exec: self.exec,
        }
    }
}

pub fn database_manager(config: Arc<Config>, log_rx: Receiver<DatabaseInput>) {
    let (mut log_db, mut flush_db) = rotate_db(&config, true);

    let mut cpt = 0;
    while let Ok(order) = log_rx.recv() {
        match order {
            DatabaseInput::Flush(entry) => flush_db.write(entry),
            DatabaseInput::Log(entry) => {
                log_db.write(entry);
                cpt += 1;
                if cpt == 500_000 {
                    info!("Rotating database...");
                    cpt = 0;
                    drop(log_db);
                    drop(flush_db);
                    (log_db, flush_db) = rotate_db(&config, false);
                    info!("Rotated database");
                }
            }
        };
    }
}

fn rotate_db(config: &Arc<Config>, startup: bool) -> (WriteDB, WriteDB) {
    let mut log_read_db = match ReadDB::open(LOG_DB_NAME) {
        Some(db) => db,
        None => {
            return (
                WriteDB::create(LOG_DB_NAME, config),
                WriteDB::create(FLUSH_DB_NAME, config),
            )
        }
    };
    let mut flush_read_db = match ReadDB::open(FLUSH_DB_NAME) {
        Some(db) => db,
        None => {
            warn!(
                "Strange! Found a {} but no {}, opening /dev/null instead",
                LOG_DB_NAME, FLUSH_DB_NAME
            );
            match ReadDB::open("/dev/null") {
                Some(db) => db,
                None => {
                    error!("Opening dummy /dev/null failed");
                    exit(1);
                }
            }
        }
    };

    let mut log_write_db = WriteDB::create(LOG_DB_NEW_NAME, config);

    _rotate_db(
        config,
        &mut log_read_db,
        &mut flush_read_db,
        &mut log_write_db,
        startup,
    );

    drop(log_read_db);
    drop(flush_read_db);

    if let Err(err) = fs::rename(LOG_DB_NEW_NAME, LOG_DB_NAME) {
        error!("Failed to replace old DB with new one: {}", err);
        exit(1);
    }

    if let Err(err) = fs::remove_file(FLUSH_DB_NAME) {
        error!("Failed to delete old DB: {}", err);
        exit(1);
    }

    let flush_write_db = WriteDB::create(FLUSH_DB_NAME, config);
    (log_write_db, flush_write_db)
}

fn _rotate_db(
    config: &Arc<Config>,
    log_read_db: &mut ReadDB,
    flush_read_db: &mut ReadDB,
    log_write_db: &mut WriteDB,
    startup: bool,
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
                if !entry.exec {
                    if entry.t + filter.retry_duration().unwrap_or_default() > now {
                        if startup {
                            // TODO send match
                        }

                        log_write_db.write(entry);
                    }
                // Replay executions & store in db
                } else if entry.t + filter.longuest_action_duration() > now {
                    if startup {
                        // TODO send flush
                    }

                    log_write_db.write(entry);
                }
            }
            Err(err) => {
                debug!("while reading log db: {}", err);
                discarded_count += 1;
            }
        }
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
