use std::{collections::BTreeMap, fmt::Debug, fs::File, io::{self, BufReader, BufWriter, Write}, process::exit};

use bincode::Options;
use chrono::{DateTime, Local};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tracing::{debug, error, warn};

use crate::{
    concepts::{ActionFilter, Config, Filter, LogEntry, Match},
    utils::{bincode_options, BincodeOptions},
};

use super::DBError;

// OPTIM Add a timestamp prefix to the header, to permit having
// shorter timestamps?
// It may permit to win 1-4 bytes per entry, don't know if it's worth it
// FIXME put signature in the header?
type DatabaseHeader = BTreeMap<usize, (String, String)>;
type ReadHeader = BTreeMap<usize, &'static Filter>;
type WriteHeader = BTreeMap<&'static Filter, usize>;

const DB_SIGNATURE: &str = "reaction-db-v01";

pub struct ReadDB {
    f: BufReader<File>,
    h: ReadHeader,
    bin: BincodeOptions,
}

impl ReadDB {
    pub fn open(path: &str, config: &'static Config) -> Result<Option<Self>, DBError> {
        let file = match File::open(path) {
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

        let mut ret = ReadDB {
            f: BufReader::new(file),
            h: BTreeMap::default(),
            bin: bincode_options(),
        };

        match ret.read::<String>() {
            Ok(signature) => {
                if DB_SIGNATURE == signature {
                    Ok(())
                } else {
                    Err(DBError::Error("database is not a reaction database".into()))
                }
            }
            Err(err) => Err(DBError::Error(format!("reading database signature: {err}"))),
        }?;

        let db_header = ret
            .read::<DatabaseHeader>()
            .map_err(|err| DBError::Error(format!("while reading database header: {err}")))?;

        ret.h = db_header
            .iter()
            .filter_map(|(key, name)| config.get_filter(name).map(|filter| (*key, filter)))
            .collect();

        Ok(Some(ret))
    }

    fn read<T: DeserializeOwned + Debug>(&mut self) -> Result<T, bincode::Error> {
        let decoded = self.bin.deserialize_from::<_, T>(&mut self.f)?;
        debug!("reading this: {:?}", &decoded);
        Ok(decoded)
    }
}

impl Iterator for ReadDB {
    type Item = Result<LogEntry, DBError>;

    fn next(&mut self) -> Option<Self::Item> {
        let res = self.read::<ComputedLogEntry>();
        match res {
            Ok(item) => Some(item.to(&self.h)),
            Err(err) => match *err {
                bincode::ErrorKind::Io(err) => match err.kind() {
                    io::ErrorKind::UnexpectedEof => None,
                    _ => Some(Err(err.into())),
                },
                _ => Some(Err(err.into())),
            },
        }
    }
}

pub struct WriteDB {
    f: BufWriter<File>,
    h: WriteHeader,
    bin: BincodeOptions,
}

impl WriteDB {
    pub fn create(path: &str, config: &'static Config) -> Self {
        let file = match File::create(path) {
            Ok(file) => file,
            Err(err) => {
                error!("Failed to create DB: {}", err);
                exit(1);
            }
        };

        let mut ret = WriteDB {
            f: BufWriter::new(file),
            h: BTreeMap::default(),
            bin: bincode_options(),
        };

        if let Err(err) = ret._write(DB_SIGNATURE) {
            error!("Failed to write to DB: {}", err);
            exit(1);
        }

        let database_header: DatabaseHeader = config
            .filters()
            .into_iter()
            .map(|f| {
                let names = f.full_name();
                (names.0.to_owned(), names.1.to_owned())
            })
            .enumerate()
            .collect();

        if let Err(err) = ret._write(&database_header) {
            error!("Failed to write to DB: {}", err);
            exit(1);
        }

        ret.h = config
            .filters()
            .into_iter()
            .enumerate()
            .map(|(key, filter)| (filter, key))
            .collect();

        ret
    }

    pub fn write(&mut self, entry: LogEntry) -> Result<(), DBError> {
        let computed = ComputedLogEntry::from(entry, &self.h)?;
        self._write(computed)
    }

    fn _write<T: Serialize + std::fmt::Debug>(&mut self, data: T) -> Result<(), DBError> {
        let encoded = self.bin.serialize(&data)?;
        // debug!("writing this: {:?}, {:?}", &data, &encoded);
        self.f.write_all(&encoded)?;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.f.flush()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ComputedLogEntry {
    pub m: Match,
    pub f: usize,
    pub t: i64,
    pub exec: bool,
}

impl ComputedLogEntry {
    fn from(value: LogEntry, header: &WriteHeader) -> Result<Self, DBError> {
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
    fn to(self, header: &ReadHeader) -> Result<LogEntry, DBError> {
        match header.get(&self.f) {
            // a timestamp will cause a panic in 300_000_000 years
            // you & me will both be dead so why bother? /o/
            #[allow(clippy::unwrap_used)]
            Some(f) => Ok(LogEntry {
                m: self.m,
                f,
                t: DateTime::from_timestamp(self.t, 0)
                    .unwrap()
                    .with_timezone(&Local),
                exec: self.exec,
            }),
            None => Err(DBError::InvalidFilterError(self.f.to_string())),
        }
    }
}
