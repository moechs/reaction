use std::time::SystemTime;

use crate::{action::ActionName, filter::FilterName};

pub type Match = Vec<String>;

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

// pub struct LogEntry {
//     pub mft: MFT,
//     pub exec: bool,
// }
