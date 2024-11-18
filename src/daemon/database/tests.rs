#![allow(clippy::unwrap_used)]
#![cfg(test)]

use std::{collections::BTreeMap, path::PathBuf};

use chrono::{Local, TimeDelta};
use rand::{
    distributions::{Alphanumeric, DistString, Uniform},
    prelude::Distribution as _,
    rngs::ThreadRng,
};
use tempfile::TempDir;
use tokio::sync::mpsc;
use tracing::Level;

use super::{ReadDB, WriteDB};
use crate::{
    concepts::{Config, Filter, Pattern, Stream},
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

fn generate_random_streams(
    rng: &mut ThreadRng,
    nb_streams: usize,
    nb_filters_per_stream: usize,
) -> BTreeMap<String, Stream> {
    let mut rng2 = rand::thread_rng();
    let nb_patterns = Uniform::new(0, 4);
    (0..nb_streams)
        .map(|_| Alphanumeric.sample_string(rng, 10))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|sname| {
            (
                sname.clone(),
                Stream::from_filters(
                    (0..nb_filters_per_stream)
                        .map(|_| Alphanumeric.sample_string(rng, 10))
                        .map(|fname| {
                            (
                                fname.clone(),
                                Filter::from_name_and_patterns(
                                    &sname,
                                    &fname,
                                    (0..nb_patterns.sample(&mut rng2))
                                        .map(|_| Pattern::default())
                                        .collect(),
                                ),
                            )
                        })
                        .collect(),
                    &sname,
                ),
            )
        })
        .collect()
}

fn generate_random_entries(
    rng: &mut ThreadRng,
    config: &'static Config,
    nb_entries: usize,
    _nb_streams: usize,
    _nb_filters_per_stream: usize,
) -> Vec<LogEntry> {
    // let rand_strings = Uniform::new(0, 5);
    // let rand_streams = Uniform::new(0, nb_streams);
    // let rand_filters = Uniform::new(0, nb_filters_per_stream);
    (0..nb_entries)
        .map(|i| {
            let f = config
                .streams()
                .iter()
                // .take(rand_streams.sample(rng))
                .take(1)
                .last()
                .unwrap()
                .1
                .filters()
                .iter()
                // .take(rand_filters.sample(rng))
                .take(1)
                .last()
                .unwrap()
                .1;

            LogEntry {
                // Random match of n Strings of size 10
                m: (0..f.patterns().len())
                    .map(|_| Alphanumeric.sample_string(rng, 10))
                    .collect(),
                // Random filter in config
                f,
                // Now + incremented microsecond (avoid duplication)
                t: Local::now() + TimeDelta::microseconds(i as i64),
                exec: false,
            }
        })
        .collect()
}

#[test]
fn write_and_read_db_big() {
    let mut rng = rand::thread_rng();

    let streams = generate_random_streams(&mut rng, 10, 10);
    let dir = TempDir::new().unwrap();
    let config = Config::from_streams(
        streams,
        &dir.into_path().into_os_string().into_string().unwrap(),
    );
    let config = Box::leak(Box::new(config));

    let entries = generate_random_entries(&mut rng, config, 300, 10, 10);

    let mut write_db = WriteDB::create(&config.path_of(DatabaseNames::LogDbName), config);

    for i in 0..entries.len() {
        assert!(
            write_db.write(entries[i].clone()).is_ok(),
            "could not write entry n°{i}"
        );
    }

    drop(write_db);

    let read_db = ReadDB::open(&config.path_of(DatabaseNames::LogDbName), config);

    assert!(read_db.is_ok());
    let read_db = read_db.unwrap();
    assert!(read_db.is_some());
    let mut read_db = read_db.unwrap();

    for i in 0..entries.len() {
        let read_entry = read_db.next();
        assert!(read_entry.is_some(), "entry n°{i} is none",);
        let read_entry = read_entry.unwrap();
        assert!(
            read_entry.is_ok(),
            "entry n°{i} is err: {}",
            read_entry.err().unwrap()
        );
        let read_entry = read_entry.unwrap();
        let entry = &entries[i];
        assert_eq!(entry.m, read_entry.m, "entry n°{i}'s match is incorrect");
        assert_eq!(
            entry.t.timestamp(),
            read_entry.t.timestamp(),
            "entry n°{i}'s t is incorrect",
        );
    }
    let read_entry = read_db.next();
    assert!(
        read_entry.is_none(),
        "entry left at end of db: {:?}",
        read_entry
    );
}

#[test]
fn rotate_db_big() {
    let mut rng = rand::thread_rng();

    let streams = generate_random_streams(&mut rng, 10, 10);
    let dir = TempDir::new().unwrap();
    let config = Config::from_streams(
        streams,
        &dir.into_path().into_os_string().into_string().unwrap(),
    );
    let config = Box::leak(Box::new(config));

    let entries = generate_random_entries(&mut rng, config, 300, 10, 10);

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

    tracing_subscriber::fmt::fmt()
        .without_time()
        .with_target(false)
        .with_max_level(Level::TRACE)
        .try_init()
        .unwrap();

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
