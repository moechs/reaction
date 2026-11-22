//! Container metadata, as seen by selectors and prefix templates.

use std::collections::BTreeMap;

/// Runtime-agnostic description of a running container.
///
/// Backends fill it from their own API. Selectors and prefix templates only
/// read this struct, so a new backend only has to map its metadata here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerMeta {
    /// Unique, stable identifier inside its backend (full Docker container ID).
    pub id: String,
    /// Human-readable name, without Docker's leading `/`.
    pub name: String,
    /// Image reference, as reported by the runtime.
    pub image: String,
    /// Container labels.
    pub labels: BTreeMap<String, String>,
    /// Backend-specific attributes exposed to templates as `{key}`.
    pub extra: BTreeMap<String, String>,
}

impl ContainerMeta {
    /// First 12 characters of the ID, like `docker ps` prints it.
    pub fn short_id(&self) -> &str {
        self.id.get(..12).unwrap_or(&self.id)
    }
}
