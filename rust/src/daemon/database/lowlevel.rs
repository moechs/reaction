use std::{collections::BTreeMap, fmt::Debug, process::exit};

use bincode::Options;
use chrono::{DateTime, Local};
use futures::{io, SinkExt, StreamExt};
use log::{debug, error, warn};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::{
    fs::File,
    io::{BufReader, BufWriter},
};
use tokio_util::{
    bytes::Bytes,
    codec::{FramedRead, FramedWrite, LengthDelimitedCodec},
};

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
    f: FramedRead<BufReader<File>, LengthDelimitedCodec>,
    h: ReadHeader,
    bin: BincodeOptions,
}

impl ReadDB {
    pub async fn open(path: &str, config: &'static Config) -> Result<Option<Self>, DBError> {
        let file = match File::open(path).await {
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
            f: FramedRead::new(BufReader::new(file), LengthDelimitedCodec::new()),
            h: BTreeMap::default(),
            bin: bincode_options(),
        };

        match ret.read::<String>().await {
            Some(Ok(signature)) => {
                if DB_SIGNATURE == signature {
                    Ok(())
                } else {
                    Err(DBError::Error("database is not a reaction database".into()))
                }
            }
            Some(Err(err)) => Err(DBError::Error(format!("reading database signature: {err}"))),
            None => Err(DBError::Error(
                "EOF while reading database header".to_string(),
            )),
        }?;

        let db_header = ret
            .read::<DatabaseHeader>()
            .await
            .unwrap_or(Err(DBError::Error(
                "EOF before database header".to_string(),
            )))
            .map_err(|err| DBError::Error(format!("while reading database header: {err}")))?;

        ret.h = db_header
            .iter()
            .filter_map(|(key, name)| config.get_filter(name).map(|filter| (*key, filter)))
            .collect();

        Ok(Some(ret))
    }

    async fn read<T: DeserializeOwned + Debug>(&mut self) -> Option<Result<T, DBError>> {
        match self.f.next().await? {
            Err(err) => Some(Err(err.into())),
            Ok(encoded) => Some({
                match self.bin.deserialize::<T>(&encoded) {
                    Ok(decoded) => {
                        debug!("reading this: {:?}", decoded);
                        Ok(decoded)
                    }
                    Err(err) => Err(err.into()),
                }
            }),
        }
    }

    // FIXME use channel instead?
    pub async fn next(&mut self) -> Option<Result<LogEntry, DBError>> {
        let res = self.read::<ComputedLogEntry>().await;
        res.map(|item| match item {
            Ok(item) => item.to(&self.h),
            Err(err) => Err(err),
        })
    }
}

pub struct WriteDB {
    f: FramedWrite<BufWriter<File>, LengthDelimitedCodec>,
    h: WriteHeader,
    bin: BincodeOptions,
}

impl WriteDB {
    pub async fn create(path: &str, config: &'static Config) -> Self {
        let file = match File::create(path).await {
            Ok(file) => file,
            Err(err) => {
                error!("Failed to create DB: {}", err);
                exit(1);
            }
        };

        let mut ret = WriteDB {
            f: FramedWrite::new(BufWriter::new(file), LengthDelimitedCodec::new()),
            h: BTreeMap::default(),
            bin: bincode_options(),
        };

        if let Err(err) = ret._write(DB_SIGNATURE).await {
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

        if let Err(err) = ret._write(&database_header).await {
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

    pub async fn write(&mut self, entry: LogEntry) -> Result<(), DBError> {
        let computed = ComputedLogEntry::from(entry, &self.h)?;
        self._write(computed).await
    }

    async fn _write<T: Serialize + std::fmt::Debug>(&mut self, data: T) -> Result<(), DBError> {
        let encoded = self.bin.serialize(&data)?;
        // debug!("writing this: {:?}, {:?}", &data, &encoded);
        self.f.send(Bytes::from(encoded)).await?;
        Ok(())
    }

    pub async fn flush(&mut self) -> io::Result<()> {
        self.f.flush().await
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
