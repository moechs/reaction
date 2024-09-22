#![cfg(test)]

use std::sync::Arc;

use chrono::Local;

use crate::database::ReadDB;
use crate::{config::Config, tests::Fixture};
use crate::{filter::Filter, messages::LogEntry};

use super::WriteDB;

#[test]
fn write_and_read_db() {
    let config_file = Fixture::from_string(
        "config.jsonnet",
        "
{
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
}
        ",
    );

    let config = Arc::new(Config::from_file(&config_file).unwrap());

    let correct_filter_name = Arc::new(Filter::from_name("stream1", "filter1"));

    let incorrect_filter_name = Arc::new(Filter::from_name("stream0", "filter1"));

    let correct_log_entry = LogEntry {
        m: vec!["match1".into()],
        f: correct_filter_name.clone(),
        t: Local::now(),
        exec: false,
    };

    let incorrect_log_entry = LogEntry {
        m: vec!["match1".into()],
        f: incorrect_filter_name.clone(),
        t: Local::now(),
        exec: false,
    };

    let db_path = Fixture::empty("matches.db");

    let mut write_db = WriteDB::create(db_path.to_str().unwrap(), &config);

    assert!(write_db.write(correct_log_entry.clone()).is_ok());
    assert!(write_db.write(incorrect_log_entry).is_err());

    drop(write_db);

    let read_db = ReadDB::open(db_path.to_str().unwrap());

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
