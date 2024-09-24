use std::{
    collections::BTreeMap,
    fmt::Debug,
    fs::File,
    io::{self, BufReader, BufWriter, Write},
    process::exit,
    sync::Arc,
};

use chrono::{DateTime, Local};
use log::{debug, error, warn};
use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    filter::Filter,
    messages::{LogEntry, Match},
};

use super::DBError;

// OPTIM Add a timestamp prefix to the header, to permit having
// shorter timestamps?
// It may permit to win 1-4 bytes per entry, don't know if it's worth it
// FIXME put signature in the header?
type DatabaseHeader = BTreeMap<usize, (String, String)>;
type ReadHeader = BTreeMap<usize, Arc<Filter>>;
type WriteHeader = BTreeMap<Arc<Filter>, usize>;

const BUFFER_MAX_SIZE: usize = 10 * 1024 * 1024;
const DB_SIGNATURE: &str = "reaction-db-v01";

pub struct ReadDB {
    f: BufReader<File>,
    h: ReadHeader,
    buf: Vec<u8>,
}

impl ReadDB {
    pub fn open(path: &str, config: &Arc<Config>) -> Result<Option<Self>, DBError> {
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
            h: BTreeMap::new(),
            buf: vec![0; BUFFER_MAX_SIZE],
        };

        match ret.read::<&str>() {
            Ok(DB_SIGNATURE) => Ok(()),
            Ok(_) => Err(DBError::Error("database is not a reaction database".into())),
            Err(err) => Err(DBError::Error(format!("reading database signature: {err}"))),
        }?;

        let db_header = ret
            .read::<DatabaseHeader>()
            .map_err(|err| DBError::Error(format!("while reading database header: {err}")))?;

        ret.h = db_header
            .iter()
            .filter_map(|(key, name)| config.get_filter(name).map(|filter| (*key, filter.clone())))
            .collect();

        Ok(Some(ret))
    }

    fn read<'a, T: Deserialize<'a> + Debug>(&'a mut self) -> Result<T, postcard::Error> {
        let (decoded, _) = postcard::from_io::<T, _>((&mut self.f, &mut self.buf))?;
        debug!("reading this: {:?}", &decoded);
        Ok(decoded)
    }
}

impl Iterator for ReadDB {
    type Item = Result<LogEntry, DBError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.read::<ComputedLogEntry>() {
            // FIXME why we got a default item instead of an error or something?
            // How do we really know we reached the end?
            // For now, checking if time is 0
            Ok(ComputedLogEntry { t: 0, .. }) => None,
            Ok(item) => Some(item.to(&self.h)),
            Err(err) => match err {
                postcard::Error::DeserializeUnexpectedEnd => None,
                _ => Some(Err(err.into())),
            },
        }
    }
}

pub struct WriteDB {
    f: BufWriter<File>,
    h: WriteHeader,
    buf: Vec<u8>,
}

impl WriteDB {
    pub fn create(path: &str, config: &Arc<Config>) -> Self {
        let file = match File::create(path) {
            Ok(file) => file,
            Err(err) => {
                error!("Failed to create DB: {}", err);
                exit(1);
            }
        };

        let mut ret = WriteDB {
            f: BufWriter::new(file),
            h: BTreeMap::new(),
            buf: vec![0; BUFFER_MAX_SIZE],
        };

        if let Err(err) = ret._write(DB_SIGNATURE) {
            error!("Failed to write to DB: {}", err);
            exit(1);
        }

        let database_header: DatabaseHeader = config
            .filters()
            .into_iter()
            .map(|f| f.full_name())
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
        let encoded = postcard::to_slice(&data, &mut self.buf)?;
        debug!("writing this: {:?}, {:?}", &data, &encoded);
        self.f.write_all(encoded)?;
        // clear
        // for i in 0..self.buf.len() {
        //     self.buf[i] = 0;
        // }
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
    #[allow(clippy::mutable_key_type)] // Interior mutability of Arc is not used
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
