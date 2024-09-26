use std::collections::{BTreeMap, BTreeSet};

use super::Match;

use serde::{Deserialize, Serialize};

// We don't need protocol versionning here because
// client and daemon are the same binary

#[derive(Clone, Serialize, Deserialize)]
pub enum ClientRequest {
    Info,
    Flush(FlushOpts),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FlushOpts {
    pub m: Match,
    pub f: (String, String),
}

#[derive(Clone, Serialize, Deserialize)]
pub enum DaemonResponse {
    Info(InfoRes),
    Flush,
    Err(String),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct InfoRes {
    pub matches: BTreeMap<(String, String), BTreeMap<Match, BTreeSet<i64>>>,
    pub execs: BTreeMap<(String, String, String), BTreeMap<Match, BTreeSet<i64>>>,
}
