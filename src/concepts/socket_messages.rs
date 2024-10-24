use std::collections::BTreeMap;

use super::Match;

use serde::{
    ser::{SerializeMap, SerializeStruct},
    Deserialize, Serialize,
};

// We don't need protocol versionning here because
// client and daemon are the same binary

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum Order {
    Show,
    Flush,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientRequest {
    pub order: Order,
    pub stream_filter: Option<String>,
    pub patterns: Vec<(String, String)>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct FlushOptions {
    pub m: Match,
    pub f: (String, String),
}

#[derive(Serialize, Deserialize)]
pub enum DaemonResponse {
    Order(ClientStatus),
    Err(String),
}

pub type ClientStatus = BTreeMap<String, BTreeMap<String, BTreeMap<String, PatternStatus>>>;

#[derive(Debug, Default, Deserialize)]
pub struct PatternStatus {
    pub matches: usize,
    pub actions: BTreeMap<String, Vec<String>>,
}

impl Serialize for PatternStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // We only skip serializing emptiness if we're on a human-readable format
        // This means we're printing for user, not exchanging it over a socket
        if serializer.is_human_readable() {
            let ser_matches = self.matches != 0;
            let ser_actions = !self.actions.is_empty();
            let mut state =
                serializer.serialize_map(Some(ser_matches as usize + ser_actions as usize))?;
            if ser_matches {
                state.serialize_entry("matches", &self.matches)?;
            }
            if ser_actions {
                state.serialize_entry("actions", &self.actions)?;
            }
            state.end()
        } else {
            let mut state = serializer.serialize_struct("PatternStatus", 2)?;
            state.serialize_field("matches", &self.matches)?;
            state.serialize_field("actions", &self.actions)?;
            state.end()
        }
    }
}
