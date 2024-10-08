use chrono::{DateTime, Local, TimeDelta};

use super::{Action, ActionFilter, Filter};

pub type Time = DateTime<Local>;
pub type Match = Vec<String>;

#[derive(Clone, Debug)]
pub struct MT<T: ActionFilter + 'static> {
    pub m: Match,
    pub o: &'static T,
    pub t: Time,
}

pub type MFT = MT<Filter>;
pub type MAT = MT<Action>;

// #[derive(Clone)]
// pub struct MFT {
//     pub m: Match,
//     pub f: &'static Filter,
//     pub t: Time,
// }

// #[derive(Clone)]
// pub struct MAT {
//     pub m: Match,
//     pub a: &'static Action,
//     pub t: Time,
// }

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub m: Match,
    pub f: &'static Filter,
    pub t: Time,
    pub exec: bool,
}

impl PartialEq for LogEntry {
    #[allow(clippy::unwrap_used)] // 1 second is obviously less seconds than i64::MAX
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
            o: value.f,
            t: value.t,
        }
    }
}
