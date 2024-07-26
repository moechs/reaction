use std::time::SystemTime;

use crate::filter::Filter;

pub type Match = Vec<String>;

pub struct PFT {
    pub m: Match,
    pub f: Filter,
    pub t: SystemTime,
}
