use chrono::{DateTime, Local, TimeDelta};

use crate::{action::ActionName, filter::FilterName};

pub type Time = DateTime<Local>;
pub type Match = Vec<String>;

#[derive(Clone)]
pub struct MFT {
    pub m: Match,
    pub f: FilterName,
    pub t: Time,
}

#[derive(Clone)]
pub struct MAT {
    pub m: Match,
    pub a: ActionName,
    pub t: Time,
}

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub m: Match,
    pub f: FilterName,
    pub t: Time,
    pub exec: bool,
}

impl PartialEq for LogEntry {
    fn eq(&self, other: &Self) -> bool {
        self.exec == other.exec
            && self.m == other.m
            && self.f == other.f
            // We loose subsecond precision while encoding LogEntry
            && (self.t - other.t) < TimeDelta::new(1, 0).unwrap()
    }
}

impl From<LogEntry> for MFT {
    fn from(value: LogEntry) -> Self {
        MFT {
            m: value.m,
            f: value.f,
            t: value.t,
        }
    }
}
