use chrono::{DateTime, Local};

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

#[derive(Clone)]
pub struct LogEntry {
    pub m: Match,
    pub f: FilterName,
    pub t: Time,
    pub exec: bool,
}
