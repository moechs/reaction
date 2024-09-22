use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{self, Write},
    process::exit,
    sync::{
        mpsc::{Receiver, Sender},
        Arc,
    },
    thread,
};

use chrono::{DateTime, Local, TimeDelta};
use log::{debug, error, info, warn};
use postcard::{from_io, Error};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    config::Config,
    filter::Filter,
    matches::MatchManagerInput,
    messages::{LogEntry, Match, Time},
};

mod tests;

const LOG_DB_NAME: &str = "./reaction-matches.db";
const LOG_DB_NEW_NAME: &str = "./reaction-matches.new.db";
const FLUSH_DB_NAME: &str = "./reaction-flushes.db";

const MAX_WRITES: u32 = 500_000;

const BUFFER_MAX_SIZE: usize = 10 * 1024 * 1024;

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

        let mut buf: Vec<u8> = vec![0; BUFFER_MAX_SIZE];
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
        let mut buf: Vec<u8> = vec![0; BUFFER_MAX_SIZE];
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
    buf: Vec<u8>,
}

impl WriteDB {
    fn create(path: &str, config: &Arc<Config>) -> Self {
        let file = match File::create(path) {
            Ok(file) => file,
            Err(err) => {
                error!("Failed to create DB: {}", err);
                exit(1);
            }
        };

        let mut ret = WriteDB {
            f: file,
            h: BTreeMap::new(),
            buf: vec![0; BUFFER_MAX_SIZE],
        };

        let database_read_header: DatabaseReadHeader =
            config.filters().into_iter().enumerate().collect();

        if let Err(err) = ret._write(&database_read_header) {
            error!("Failed to write to DB: {}", err);
            exit(1);
        }

        let database_write_header = database_read_header
            .into_iter()
            .map(|(i, name)| (name, i))
            .collect();

        ret.h = database_write_header;
        ret
    }

    fn write(&mut self, entry: LogEntry) -> Result<(), DBError> {
        let computed = ComputedLogEntry::from(entry, &self.h)?;
        self._write(computed)
    }

    fn _write<T: Serialize + std::fmt::Debug>(&mut self, data: T) -> Result<(), DBError> {
        let encoded = postcard::to_slice(&data, &mut self.buf)?;
        debug!("writing this: {:?}, {:?}", &data, &encoded);
        self.f.write_all(encoded)?;
        // clear
        for i in 0..self.buf.len() {
            self.buf[i] = 0;
        }
        Ok(())
    }
}

#[derive(Error, Debug)]
enum DBError {
    #[error("invalid filter: {0}")]
    InvalidFilterError(String),
    #[error("decode error: {0}")]
    PostcardError(#[from] postcard::Error),
    #[error("io error: {0}")]
    IOError(#[from] io::Error),
    #[error("{0}")]
    Error(String),
}

type DatabaseWriteHeader = BTreeMap<Arc<Filter>, usize>;
type DatabaseReadHeader = BTreeMap<usize, Arc<Filter>>;

#[derive(Debug, Serialize, Deserialize)]
struct ComputedLogEntry {
    pub m: Match,
    pub f: usize,
    pub t: i64,
    pub exec: bool,
}

impl ComputedLogEntry {
    fn from(value: LogEntry, header: &DatabaseWriteHeader) -> Result<Self, DBError> {
        let filter = value.f;
        match header.get(&filter) {
            Some(f) => Ok(ComputedLogEntry {
                m: value.m,
                f: *f,
                t: value.t.timestamp(),
                exec: value.exec,
            }),
            None => Err(DBError::InvalidFilterError(filter.to_string())),
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
    #[allow(dead_code)]
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
        for order in log_rx.iter() {
            match order {
                DatabaseManagerInput::Flush(entry) => write_or_die!(flush_db, entry),
                DatabaseManagerInput::Log(entry) => {
                    write_or_die!(log_db, entry);
                    cpt += 1;
                    if cpt == MAX_WRITES {
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
    info!("Rotating database...");
    let res = _rotate_db(config, &matches_tx);

    debug!("sending EOS: {}", matches_tx.is_some());
    if let Some(tx) = matches_tx {
        tx.send(MatchManagerInput::EndOfStartup).unwrap();
    }

    info!("Rotated database");
    res
}
fn _rotate_db(
    config: &Arc<Config>,
    matches_tx: &Option<Sender<MatchManagerInput>>,
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

    __rotate_db(
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

fn __rotate_db(
    matches_tx: &Option<Sender<MatchManagerInput>>,
    log_read_db: &mut ReadDB,
    flush_read_db: &mut ReadDB,
    log_write_db: &mut WriteDB,
) {
    let mut discarded_errors: HashMap<String, usize> = HashMap::new();

    let mut millisecond_disambiguation_counter: u32 = 0;

    // Read flushes
    let mut flushes: BTreeMap<Arc<Filter>, BTreeMap<Match, Time>> = BTreeMap::new();
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
    let last_global_flush = flushes.get(&Filter::default());

    let now = Local::now();

    // Read matches
    for log_entry in log_read_db {
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
                if (!entry.exec && entry.t + entry.f.retry_duration().unwrap_or_default() > now)
                    || (entry.exec && entry.t + entry.f.longuest_action_duration() > now)
                {
                    // We loose subsecond precision when storing times, so we add those fake
                    // milliseconds to make sure each time is unique
                    entry.t += TimeDelta::new(0, millisecond_disambiguation_counter).unwrap();
                    millisecond_disambiguation_counter += 1;

                    if let Some(tx) = matches_tx {
                        tx.send(MatchManagerInput::Match(entry.clone().into()))
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
