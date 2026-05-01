use std::collections::BTreeMap;

use serde::{
    // ser::{SerializeMap, SerializeStruct},
    Deserialize,
    Serialize,
};

// We don't need protocol versionning here because
// client and daemon are the same binary

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum Order {
    Show,
    Flush,
    Trigger,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientRequest {
    pub order: Order,
    pub stream_filter: Option<String>,
    pub patterns: Vec<(String, String)>,
}

#[derive(Serialize, Deserialize)]
pub enum DaemonResponse {
    Ok(()),
    Order(ClientStatus),
    Err(String),
}

pub type ClientStatus = BTreeMap<String, BTreeMap<String, BTreeMap<String, PatternStatus>>>;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PatternStatus {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub matches: usize,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<String, Vec<String>>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

pub trait Cleanable {
    /// Remove empty fields
    fn clean(self) -> Self;
}

impl Cleanable for ClientStatus {
    fn clean(self) -> Self {
        self.into_iter()
            .map(|(key, value)| {
                (
                    key,
                    value
                        .into_iter()
                        .map(|(key, value)| {
                            (
                                key,
                                value
                                    .into_iter()
                                    .filter(|(_, value)| {
                                        value.matches != 0 || !value.actions.is_empty()
                                    })
                                    .collect::<BTreeMap<_, _>>(),
                            )
                        })
                        .filter(|(_, value)| !value.is_empty())
                        .collect::<BTreeMap<_, _>>(),
                )
            })
            .filter(|(_, value)| !value.is_empty())
            .collect()
    }
}
