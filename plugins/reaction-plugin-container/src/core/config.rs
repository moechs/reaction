//! Stream options shared by every backend (`streams.<name>.options`).

use std::time::{Duration, SystemTime};

use reaction_plugin::{Value, time::parse_duration};
use serde::{Deserialize, Deserializer};

use super::{
    runtime::Source,
    selector::{Filter, Selector},
    template::Template,
};

pub const DEFAULT_MAX_LINE_BYTES: usize = 64 * 1024;
/// Upper bound for `max_line_bytes`: each line crosses the remoc channel in one message.
pub const MAX_MAX_LINE_BYTES: usize = 1024 * 1024;

/// Where to start reading the logs of a container when it is first seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Since {
    /// Only lines written after the plugin started.
    #[default]
    Now,
    /// Also lines written up to this long before the plugin started.
    Ago(Duration),
}

impl Since {
    pub fn baseline(self, started_at: SystemTime) -> SystemTime {
        match self {
            Since::Now => started_at,
            // SystemTime can represent pre-epoch instants on Unix: clamp explicitly.
            Since::Ago(d) => started_at
                .checked_sub(d)
                .filter(|t| *t >= SystemTime::UNIX_EPOCH)
                .unwrap_or(SystemTime::UNIX_EPOCH),
        }
    }
}

impl<'de> Deserialize<'de> for Since {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s.trim() == "now" {
            Ok(Since::Now)
        } else {
            parse_duration(&s).map(Since::Ago).map_err(|err| {
                serde::de::Error::custom(format!("since: expected \"now\" or a duration: {err}"))
            })
        }
    }
}

/// Which time reaction receives with each line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimestampMode {
    /// The time the plugin received the line. Same behavior as `cmd` streams.
    /// Keeps time monotonic when several containers feed one stream.
    #[default]
    Receive,
    /// The timestamp reported by the runtime (falls back to receive time).
    Runtime,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStreamOptions {
    #[serde(rename = "match", default)]
    include: Selector,
    #[serde(default)]
    exclude: Selector,
    #[serde(default = "default_sources")]
    sources: Vec<Source>,
    #[serde(default)]
    since: Since,
    #[serde(default)]
    timestamp: TimestampMode,
    #[serde(default)]
    prefix: String,
    #[serde(default = "default_max_line_bytes")]
    max_line_bytes: usize,
}

fn default_sources() -> Vec<Source> {
    vec![Source::Stdout, Source::Stderr]
}
fn default_max_line_bytes() -> usize {
    DEFAULT_MAX_LINE_BYTES
}

/// Validated options of one stream.
#[derive(Debug, Clone)]
pub struct StreamOptions {
    pub filter: Filter,
    pub stdout: bool,
    pub stderr: bool,
    pub since: Since,
    pub timestamp: TimestampMode,
    pub prefix: Template,
    pub max_line_bytes: usize,
}

impl StreamOptions {
    /// Parse and validate. `extra_keys` are the backend-specific template variables.
    pub fn parse(stream_name: &str, config: Value, extra_keys: &[&str]) -> Result<Self, String> {
        Self::_parse(config, extra_keys).map_err(|err| format!("stream {stream_name}: {err}"))
    }

    fn _parse(config: Value, extra_keys: &[&str]) -> Result<Self, String> {
        if matches!(config, Value::Null) {
            return Err("options are required: at least options.match must be set".into());
        }
        let raw: RawStreamOptions = serde_json::from_value(config.into())
            .map_err(|err| format!("invalid options: {err}"))?;

        if raw.include.is_empty() {
            return Err("options.match must contain at least one name or label".into());
        }
        raw.include.validate("match")?;
        raw.exclude.validate("exclude")?;

        if raw.sources.is_empty() {
            return Err("options.sources must not be empty".into());
        }
        if raw.max_line_bytes == 0 || raw.max_line_bytes > MAX_MAX_LINE_BYTES {
            return Err(format!(
                "options.max_line_bytes must be between 1 and {MAX_MAX_LINE_BYTES}"
            ));
        }

        Ok(Self {
            filter: Filter {
                include: raw.include,
                exclude: raw.exclude,
            },
            stdout: raw.sources.contains(&Source::Stdout),
            stderr: raw.sources.contains(&Source::Stderr),
            since: raw.since,
            timestamp: raw.timestamp,
            prefix: Template::parse(&raw.prefix, extra_keys)?,
            max_line_bytes: raw.max_line_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(v: serde_json::Value) -> Result<StreamOptions, String> {
        StreamOptions::parse("s", v.into(), &[])
    }

    #[test]
    fn defaults() {
        let o = parse(json!({"match": {"names": ["web"]}})).unwrap();
        assert!(o.stdout && o.stderr);
        assert_eq!(o.since, Since::Now);
        assert_eq!(o.timestamp, TimestampMode::Receive);
        assert!(o.prefix.is_empty());
        assert_eq!(o.max_line_bytes, DEFAULT_MAX_LINE_BYTES);
    }

    #[test]
    fn full() {
        let o = parse(json!({
            "match": {"names": ["web"], "labels": ["app=shop", "tier"]},
            "exclude": {"labels": ["reaction.ignore"]},
            "sources": ["stderr"],
            "since": "10m",
            "timestamp": "runtime",
            "prefix": "{name} ",
            "max_line_bytes": 100,
        }))
        .unwrap();
        assert!(!o.stdout && o.stderr);
        assert_eq!(o.since, Since::Ago(Duration::from_secs(600)));
        assert_eq!(o.timestamp, TimestampMode::Runtime);
        assert_eq!(o.max_line_bytes, 100);
        assert_eq!(o.filter.include.labels.len(), 2);
    }

    #[test]
    fn errors() {
        for bad in [
            json!(null),
            json!({}),
            json!({"match": {}}),
            json!({"match": {"names": [""]}}),
            json!({"match": {"names": ["a"]}, "unknown": 1}),
            json!({"match": {"nams": ["a"]}}),
            json!({"match": {"labels": ["=x"]}}),
            json!({"match": {"names": ["a"]}, "sources": []}),
            json!({"match": {"names": ["a"]}, "sources": ["stdin"]}),
            json!({"match": {"names": ["a"]}, "since": "yesterday"}),
            json!({"match": {"names": ["a"]}, "timestamp": "container"}),
            json!({"match": {"names": ["a"]}, "prefix": "{nope}"}),
            json!({"match": {"names": ["a"]}, "max_line_bytes": 0}),
        ] {
            assert!(parse(bad.clone()).is_err(), "should fail: {bad}");
        }
    }

    #[test]
    fn baseline() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        assert_eq!(Since::Now.baseline(now), now);
        assert_eq!(
            Since::Ago(Duration::from_secs(10)).baseline(now),
            now - Duration::from_secs(10)
        );
        assert_eq!(
            Since::Ago(Duration::from_secs(5000)).baseline(now),
            SystemTime::UNIX_EPOCH
        );
    }
}
