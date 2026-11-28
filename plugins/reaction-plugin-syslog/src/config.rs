//! Plugin and stream configuration parsing.

use std::{net::SocketAddr, time::Duration};

use reaction_plugin::{Value as PluginValue, time::deser_duration};
use serde::{Deserialize, Deserializer};
use serde_json::Value as JsonValue;

use crate::{
    message::{Facility, Severity, SyslogMeta},
    template::Template,
};

const MAX_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(3600);
const MAX_TIMESTAMP_SKEW: Duration = Duration::from_secs(3600);
const DEFAULT_MAX_TCP_CONNECTIONS: usize = 128;
const MAX_TCP_CONNECTIONS: usize = 4096;
const DEFAULT_TCP_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Udp,
    Tcp,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParseMode {
    #[default]
    Auto,
    Rfc3164,
    Rfc5424,
    Raw,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimestampMode {
    #[default]
    Receive,
    Message,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Framing {
    #[default]
    Auto,
    Newline,
    Octet,
}

fn default_reconnect_delay() -> Duration {
    Duration::from_secs(5)
}

fn default_max_timestamp_skew() -> Duration {
    Duration::from_secs(5)
}

fn default_max_tcp_connections() -> usize {
    DEFAULT_MAX_TCP_CONNECTIONS
}

fn default_tcp_idle_timeout() -> Duration {
    DEFAULT_TCP_IDLE_TIMEOUT
}

fn default_max_datagram_bytes() -> usize {
    65535
}

fn default_max_line_bytes() -> usize {
    65536
}

fn deserialize_socket_addr<'de, D>(deserializer: D) -> Result<SocketAddr, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    value.parse().map_err(serde::de::Error::custom)
}

/// `plugins.<name>.options`
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginOptions {
    #[serde(deserialize_with = "deserialize_socket_addr")]
    pub listen: SocketAddr,
    #[serde(
        default = "default_reconnect_delay",
        deserialize_with = "deser_duration"
    )]
    pub reconnect_delay: Duration,
    #[serde(default)]
    pub protocol: Protocol,
    #[serde(default)]
    pub parse: ParseMode,
    #[serde(default = "default_max_datagram_bytes")]
    pub max_datagram_bytes: usize,
    #[serde(default = "default_max_line_bytes")]
    pub max_line_bytes: usize,
    #[serde(default)]
    pub timestamp: TimestampMode,
    #[serde(default)]
    pub framing: Framing,
    #[serde(
        default = "default_max_timestamp_skew",
        deserialize_with = "deser_duration"
    )]
    pub max_timestamp_skew: Duration,
    #[serde(default = "default_max_tcp_connections")]
    pub max_tcp_connections: usize,
    #[serde(
        default = "default_tcp_idle_timeout",
        deserialize_with = "deser_duration"
    )]
    pub tcp_idle_timeout: Duration,
}

impl Default for PluginOptions {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:5140".parse().expect("constant address"),
            reconnect_delay: default_reconnect_delay(),
            protocol: Protocol::Udp,
            parse: ParseMode::Auto,
            max_datagram_bytes: default_max_datagram_bytes(),
            max_line_bytes: default_max_line_bytes(),
            timestamp: TimestampMode::Receive,
            framing: Framing::Auto,
            max_timestamp_skew: default_max_timestamp_skew(),
            max_tcp_connections: default_max_tcp_connections(),
            tcp_idle_timeout: default_tcp_idle_timeout(),
        }
    }
}

impl PluginOptions {
    pub fn parse(options: Vec<PluginValue>) -> Result<Self, String> {
        let merged = options
            .into_iter()
            .try_fold(PluginValue::Null, |acc, value| acc.merge(value))?;
        let parsed: Self = serde_json::from_value(merged.into())
            .map_err(|err| format!("invalid plugin options: {err}"))?;
        parsed.validate()?;
        Ok(parsed)
    }

    fn validate(&self) -> Result<(), String> {
        if self.reconnect_delay.is_zero() {
            return Err("reconnect_delay must be greater than 0".into());
        }
        if self.reconnect_delay > MAX_RECONNECT_DELAY {
            return Err(format!(
                "reconnect_delay must not exceed {}s",
                MAX_RECONNECT_DELAY.as_secs()
            ));
        }
        if self.max_timestamp_skew > MAX_TIMESTAMP_SKEW {
            return Err(format!(
                "max_timestamp_skew must not exceed {}s",
                MAX_TIMESTAMP_SKEW.as_secs()
            ));
        }
        if self.tcp_idle_timeout.is_zero() || self.tcp_idle_timeout > MAX_RECONNECT_DELAY {
            return Err(format!(
                "tcp_idle_timeout must be between 1s and {}s",
                MAX_RECONNECT_DELAY.as_secs()
            ));
        }
        if self.max_tcp_connections == 0 || self.max_tcp_connections > MAX_TCP_CONNECTIONS {
            return Err(format!(
                "max_tcp_connections must be between 1 and {MAX_TCP_CONNECTIONS}"
            ));
        }
        for (name, value) in [
            ("max_datagram_bytes", self.max_datagram_bytes),
            ("max_line_bytes", self.max_line_bytes),
        ] {
            if value == 0 || value > MAX_BUFFER_BYTES {
                return Err(format!("{name} must be between 1 and {MAX_BUFFER_BYTES}"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selectors {
    #[serde(default)]
    pub facilities: Vec<String>,
    #[serde(default)]
    pub severities: Vec<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub appnames: Vec<String>,
    #[serde(default)]
    pub procids: Vec<String>,
}

impl Selectors {
    fn normalize(&mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        self.facilities = self
            .facilities
            .iter()
            .filter_map(|value| match Facility::parse(value) {
                Some(facility) => Some(facility.as_str().to_string()),
                None => {
                    errors.push(format!("select.facilities has invalid value {value:?}"));
                    None
                }
            })
            .collect();
        self.severities = self
            .severities
            .iter()
            .filter_map(|value| match Severity::parse(value) {
                Some(severity) => Some(severity.as_str().to_string()),
                None => {
                    errors.push(format!("select.severities has invalid value {value:?}"));
                    None
                }
            })
            .collect();
        for (name, values) in [
            ("hosts", &mut self.hosts),
            ("appnames", &mut self.appnames),
            ("procids", &mut self.procids),
        ] {
            if values.iter().any(String::is_empty) {
                errors.push(format!("select.{name} has an empty value"));
            }
            for item in values.iter_mut() {
                *item = item.to_ascii_lowercase();
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub fn matches(&self, meta: &SyslogMeta) -> bool {
        if !self.facilities.is_empty()
            && !meta
                .facility
                .is_some_and(|facility| self.facilities.iter().any(|v| v == facility.as_str()))
        {
            return false;
        }
        if !self.severities.is_empty()
            && !meta
                .severity
                .is_some_and(|severity| self.severities.iter().any(|v| v == severity.as_str()))
        {
            return false;
        }
        if !matches_value(&self.hosts, meta.host.as_deref()) {
            return false;
        }
        if !matches_value(&self.appnames, meta.appname.as_deref()) {
            return false;
        }
        matches_value(&self.procids, meta.procid.as_deref())
    }
}

fn matches_value(values: &[String], actual: Option<&str>) -> bool {
    if values.is_empty() {
        return true;
    }
    actual.is_some_and(|actual| {
        let actual = actual.to_ascii_lowercase();
        values.iter().any(|value| value == &actual)
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamOptions {
    #[serde(default)]
    pub select: Selectors,
    #[serde(default)]
    pub prefix: String,
}

impl StreamOptions {
    pub fn parse(stream_name: &str, config: PluginValue) -> Result<Self, String> {
        let config: JsonValue = config.into();
        let mut options: Self = serde_json::from_value(config)
            .map_err(|err| format!("stream {stream_name}: invalid options: {err}"))?;
        options
            .select
            .normalize()
            .map_err(|err| format!("stream {stream_name}: {err}"))?;
        Template::parse(&options.prefix)
            .map_err(|err| format!("stream {stream_name}: invalid prefix: {err}"))?;
        Ok(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_options(value: JsonValue) -> Vec<PluginValue> {
        vec![value.into()]
    }

    #[test]
    fn parse_plugin_options() {
        let options = PluginOptions::parse(plugin_options(serde_json::json!({
            "listen": "127.0.0.1:5140"
        })))
        .unwrap();
        assert_eq!(options.protocol, Protocol::Udp);
        assert_eq!(options.parse, ParseMode::Auto);
        assert_eq!(options.timestamp, TimestampMode::Receive);
        assert_eq!(options.max_datagram_bytes, 65535);
        assert_eq!(options.reconnect_delay, Duration::from_secs(5));
        assert_eq!(options.framing, Framing::Auto);
        assert_eq!(options.max_timestamp_skew, Duration::from_secs(5));
        assert_eq!(options.max_tcp_connections, 128);
        assert_eq!(options.tcp_idle_timeout, Duration::from_secs(60));
    }

    #[test]
    fn reject_invalid_plugin_options() {
        assert!(PluginOptions::parse(vec![]).is_err());
        assert!(
            PluginOptions::parse(plugin_options(serde_json::json!({
                "listen": "not-an-address"
            })))
            .is_err()
        );
        assert!(
            PluginOptions::parse(plugin_options(serde_json::json!({
                "listen": "127.0.0.1:5140",
                "protocol": "sctp"
            })))
            .is_err()
        );
        assert!(
            PluginOptions::parse(plugin_options(serde_json::json!({
                "listen": "127.0.0.1:5140",
                "max_line_bytes": 0
            })))
            .is_err()
        );
        assert!(
            PluginOptions::parse(plugin_options(serde_json::json!({
                "listen": "127.0.0.1:5140",
                "reconnect_delay": "0s"
            })))
            .is_err()
        );
        assert!(
            PluginOptions::parse(plugin_options(serde_json::json!({
                "listen": "127.0.0.1:5140",
                "reconnect_delay": "3601s"
            })))
            .is_err()
        );
        assert!(
            PluginOptions::parse(plugin_options(serde_json::json!({
                "listen": "127.0.0.1:5140",
                "max_timestamp_skew": "3601s"
            })))
            .is_err()
        );
        assert!(
            PluginOptions::parse(plugin_options(serde_json::json!({
                "listen": "127.0.0.1:5140",
                "tcp_idle_timeout": "0s"
            })))
            .is_err()
        );
        assert!(
            PluginOptions::parse(plugin_options(serde_json::json!({
                "listen": "127.0.0.1:5140",
                "max_tcp_connections": 4097
            })))
            .is_err()
        );
    }

    #[test]
    fn parse_stream_options() {
        let options = StreamOptions::parse(
            "auth",
            serde_json::json!({
                "select": {
                    "facilities": ["authpriv"],
                    "severities": ["warn"]
                },
                "prefix": "{host} "
            })
            .into(),
        )
        .unwrap();
        assert_eq!(options.select.facilities, ["authpriv"]);
        assert_eq!(options.select.severities, ["warning"]);
    }

    #[test]
    fn reject_invalid_stream_options() {
        assert!(StreamOptions::parse("auth", serde_json::json!({"unknown": true}).into()).is_err());
        assert!(
            StreamOptions::parse("auth", serde_json::json!({"prefix": "{unknown}"}).into())
                .is_err()
        );
        assert!(
            StreamOptions::parse(
                "auth",
                serde_json::json!({"select": {"facilities": ["atuh"]}}).into()
            )
            .is_err()
        );
        assert!(
            StreamOptions::parse(
                "auth",
                serde_json::json!({"select": {"severities": ["crit", "bogus"]}}).into()
            )
            .is_err()
        );
        assert!(
            StreamOptions::parse(
                "auth",
                serde_json::json!({"select": {"hosts": [""]}}).into()
            )
            .is_err()
        );
    }
}
