#![allow(clippy::unwrap_used)]
#![cfg(test)]

use std::{collections::BTreeMap, path::PathBuf};

use chrono::{Local, TimeDelta};
use rand::{
    distributions::{Alphanumeric, DistString, Uniform},
    prelude::Distribution as _,
};
use tempfile::TempDir;
use tokio::sync::mpsc;

use super::{ReadDB, WriteDB};
use crate::{
    concepts::{Config, Filter, Stream},
    daemon::database::{rotate_db, DatabaseNames, LogEntry},
    tests::Fixture,
};

const BASIC_INNER_CONF: &str = "
    patterns: {
        num: { regex: '[0-9]+' },
    },
    streams: {
        stream1: {
            cmd: ['sh', '-c', 'for i in $(seq 10); do echo $((i % 5)); done'],
            filters: {
                filter1: {
                    regex: ['<num>'],
                    retry: 2,
                    retryperiod: '5s',
                    actions: {
                        action1: {
                            cmd: ['echo', '<num>'],
                        }
                    }
                }
            }
        }
    }
";

#[test]
fn db_name() {
    fn eq(path: PathBuf, str: &str) {
        assert_eq!(path.into_os_string().into_string().unwrap(), str);
    }

    let config_file = Fixture::from_string(
        "config.jsonnet",
        &format!(
            "{{ {}, state_directory: '/var/lib/reaction' }}",
            BASIC_INNER_CONF
        ),
    );
    let config = Config::from_file(&config_file).unwrap();
    assert_eq!(config.state_directory(), "/var/lib/reaction");
    eq(
        config.path_of(DatabaseNames::LogDbName),
        "/var/lib/reaction/reaction-matches.db",
    );
    eq(
        config.path_of(DatabaseNames::LogDbNewName),
        "/var/lib/reaction/reaction-matches.new.db",
    );
    eq(
        config.path_of(DatabaseNames::FlushDbName),
        "/var/lib/reaction/reaction-flushes.db",
    );

    let config_file =
        Fixture::from_string("config.jsonnet", &format!("{{ {} }}", BASIC_INNER_CONF));
    let config = Config::from_file(&config_file).unwrap();
    assert_eq!(config.state_directory(), "");
    eq(
        config.path_of(DatabaseNames::LogDbName),
        "reaction-matches.db",
    );
    eq(
        config.path_of(DatabaseNames::LogDbNewName),
        "reaction-matches.new.db",
    );
    eq(
        config.path_of(DatabaseNames::FlushDbName),
        "reaction-flushes.db",
    );
}

#[test]
fn write_and_read_db() {
    let dir = TempDir::new().unwrap();

    let config_file = Fixture::from_string(
        "config.jsonnet",
        &format!(
            "{{ {}, state_directory: '{}' }}",
            BASIC_INNER_CONF,
            dir.into_path().into_os_string().into_string().unwrap()
        ),
    );

    let config = Box::leak(Box::new(Config::from_file(&config_file).unwrap()));

    let correct_filter_name = Box::leak(Box::new(Filter::from_name("stream1", "filter1")));

    let incorrect_filter_name = Box::leak(Box::new(Filter::from_name("stream0", "filter1")));

    let correct_log_entry = LogEntry {
        m: vec!["match1".into()],
        f: correct_filter_name,
        t: Local::now(),
        exec: false,
    };

    let incorrect_log_entry = LogEntry {
        m: vec!["match1".into()],
        f: incorrect_filter_name,
        t: Local::now(),
        exec: false,
    };

    let mut write_db = WriteDB::create(&config.path_of(DatabaseNames::LogDbName), config);

    assert!(write_db.write(correct_log_entry.clone()).is_ok());
    assert!(write_db.write(incorrect_log_entry).is_err());

    drop(write_db);

    let read_db = ReadDB::open(&config.path_of(DatabaseNames::LogDbName), config);

    assert!(read_db.is_ok());
    let read_db = read_db.unwrap();
    assert!(read_db.is_some());
    let mut read_db = read_db.unwrap();

    let read_entry = read_db.next();
    assert!(read_entry.is_some());
    let read_entry = read_entry.unwrap();
    assert!(read_entry.is_ok());
    assert_eq!(read_entry.unwrap(), correct_log_entry);

    let read_entry = read_db.next();
    assert!(read_entry.is_none());
}

#[test]
fn write_and_read_db_big() {
    let mut rng = rand::thread_rng();

    let streams = (0..10)
        .map(|_| Alphanumeric.sample_string(&mut rng, 10))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|sname| {
            (
                sname.clone(),
                Stream::from_filters(
                    (0..10)
                        .map(|_| Alphanumeric.sample_string(&mut rng, 10))
                        .map(|fname| (fname.clone(), Filter::from_name(&sname, &fname)))
                        .collect(),
                    &sname,
                ),
            )
        })
        .collect();

    let dir = TempDir::new().unwrap();
    let config = Config::from_streams(
        streams,
        &dir.into_path().into_os_string().into_string().unwrap(),
    );
    let config = Box::leak(Box::new(config));

    let u0t5 = Uniform::new(0, 5);
    // let u0t10 = Uniform::new(0, 9);

    // 300 random entries
    let entries: Vec<_> = (0..300)
        .map(|i| LogEntry {
            // Random match of 5 Strings of size 10
            m: (0..u0t5.sample(&mut rng))
                .map(|_| Alphanumeric.sample_string(&mut rng, 10))
                .collect(),
            // Random filter in config
            f: config
                .streams()
                .iter()
                // .take(u0t10.sample(&mut rng))
                .take(1)
                .last()
                .unwrap()
                .1
                .filters()
                .iter()
                // .take(u0t10.sample(&mut rng))
                .take(1)
                .last()
                .unwrap()
                .1,
            // Now + incremented microsecond (avoid duplication)
            t: Local::now() + TimeDelta::microseconds(i),
            exec: false,
        })
        .collect();

    let mut write_db = WriteDB::create(&config.path_of(DatabaseNames::LogDbName), config);
    {
        let mut _flush_db = WriteDB::create(&config.path_of(DatabaseNames::FlushDbName), config);
    }

    for i in 0..entries.len() {
        assert!(
            write_db.write(entries[i].clone()).is_ok(),
            "could not write entry n°{i}"
        );
    }

    drop(write_db);

    let mut log2filter_tx = BTreeMap::new();
    let mut log2filter_rx = BTreeMap::new();
    for stream in config.streams().values() {
        for filter in stream.filters().values() {
            let (tx, rx) = mpsc::channel(3000);
            log2filter_tx.insert(filter, tx);
            log2filter_rx.insert(filter, rx);
        }
    }

    // We clone the senders so that the channels are not closed after database rotation
    let _log2filter_tx = log2filter_tx.clone();

    let rotated = rotate_db(config, Some(log2filter_tx));
    assert!(
        rotated.is_ok(),
        "database rotation failed: {}",
        rotated.err().unwrap()
    );

    for i in 0..entries.len() {
        let entry = &entries[i];
        let rx = &mut log2filter_rx.get_mut(entry.f).unwrap();
        let read_entry = rx.try_recv();
        assert!(
            read_entry.is_ok(),
            "entry n°{i} is err: {}",
            read_entry.err().unwrap()
        );
        let (m, t) = read_entry.unwrap();
        assert_eq!(entry.m, m, "entry n°{i}'s match is incorrect");
        assert_eq!(
            t.timestamp(),
            entry.t.timestamp(),
            "entry n°{i}'s t is incorrect",
        );
    }

    for (_, rx) in &log2filter_rx {
        assert!(rx.is_empty());
    }
}
