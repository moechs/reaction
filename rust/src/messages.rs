use std::time::SystemTime;

use crate::{action::ActionName, filter::FilterName};

pub type Match = Vec<String>;

#[derive(Clone)]
pub struct MFT {
    pub m: Match,
    pub f: FilterName,
    pub t: SystemTime,
}

pub struct MAT {
    pub m: Match,
    pub a: ActionName,
    pub t: SystemTime,
}

#[allow(dead_code)]
pub struct LogEntry {
    pub mft: MFT,
    pub exec: bool,
}
