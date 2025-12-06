mod action;
mod config;
mod filter;
mod pattern;
mod plugin;
mod stream;

use std::{
    fmt::{self, Debug},
    ops::{Add, Deref, Sub},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub use action::Action;
pub use config::{Config, Patterns};
pub use filter::{Duplicate, Filter};
pub use pattern::{Pattern, PatternType};
pub use plugin::Plugin;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use stream::Stream;

/// [`std::time::Duration`] since [`std::time::UNIX_EPOCH`]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Time(Duration);
impl Deref for Time {
    type Target = Duration;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl From<Duration> for Time {
    fn from(value: Duration) -> Self {
        Time(value)
    }
}
impl Into<Duration> for Time {
    fn into(self) -> Duration {
        self.0
    }
}
impl Add<Duration> for Time {
    type Output = Time;
    fn add(self, rhs: Duration) -> Self::Output {
        Time(self.0 + rhs)
    }
}
impl Add<Time> for Time {
    type Output = Time;
    fn add(self, rhs: Time) -> Self::Output {
        Time(self.0 + rhs.0)
    }
}
impl Sub<Duration> for Time {
    type Output = Time;
    fn sub(self, rhs: Duration) -> Self::Output {
        Time(self.0 - rhs)
    }
}
impl Sub<Time> for Time {
    type Output = Time;
    fn sub(self, rhs: Time) -> Self::Output {
        Time(self.0 - rhs.0)
    }
}

impl Serialize for Time {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.as_nanos().to_string().serialize(serializer)
    }
}
struct TimeVisitor;
impl<'de> serde::de::Visitor<'de> for TimeVisitor {
    type Value = Time;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "a string representing nanoseconds")
    }

    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        match s.parse::<u128>() {
            Ok(nanos) => Ok(Time(Duration::new(
                (nanos / 1_000_000_000) as u64,
                (nanos % 1_000_000_000) as u32,
            ))),
            Err(_) => Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(s),
                &self,
            )),
        }
    }
}
impl<'de> Deserialize<'de> for Time {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(TimeVisitor)
    }
}

impl Time {
    pub fn new(secs: u64, nanos: u32) -> Time {
        Time(Duration::new(secs, nanos))
    }
    pub fn from_hours(hours: u64) -> Time {
        Time(Duration::from_hours(hours))
    }
    pub fn from_mins(mins: u64) -> Time {
        Time(Duration::from_mins(mins))
    }
    pub fn from_secs(secs: u64) -> Time {
        Time(Duration::from_secs(secs))
    }
    pub fn from_millis(millis: u64) -> Time {
        Time(Duration::from_millis(millis))
    }
    pub fn from_nanos(nanos: u64) -> Time {
        Time(Duration::from_nanos(nanos))
    }
}

pub fn now() -> Time {
    Time(SystemTime::now().duration_since(UNIX_EPOCH).unwrap())
}

pub type Match = Vec<String>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MatchTime {
    pub m: Match,
    pub t: Time,
}

fn merge_attrs<A: Default + Debug + PartialEq + Eq + Clone>(
    this: A,
    other: A,
    default: A,
    name: &str,
) -> Result<A, String> {
    if !(this == default || other == default || this == other) {
        return Err(format!(
            "'{name}' has conflicting definitions: '{this:?}', '{other:?}'"
        ));
    }
    if this == default {
        return Ok(other);
    }
    Ok(this)
}

fn null_value() -> Value {
    Value::Null
}

#[cfg(test)]
pub use filter::tests as filter_tests;

#[cfg(test)]
mod tests {
    use crate::concepts::merge_attrs;

    #[test]
    fn test_merge_attrs() {
        assert_eq!(merge_attrs(None::<String>, None, None, "t"), Ok(None));
        assert_eq!(
            merge_attrs(Some("coucou"), None, None, "t"),
            Ok(Some("coucou"))
        );
        assert_eq!(
            merge_attrs(None, Some("coucou"), None, "t"),
            Ok(Some("coucou"))
        );
        assert_eq!(
            merge_attrs(Some("coucou"), Some("coucou"), None, "t"),
            Ok(Some("coucou"))
        );
        assert_eq!(
            merge_attrs(Some("coucou"), Some("hello"), None, "t"),
            Err("'t' has conflicting definitions: 'Some(\"coucou\")', 'Some(\"hello\")'".into())
        );

        assert_eq!(merge_attrs("", "", "", "t"), Ok(""));
        assert_eq!(merge_attrs("coucou", "", "", "t"), Ok("coucou"));
        assert_eq!(merge_attrs("", "coucou", "", "t"), Ok("coucou"));
        assert_eq!(merge_attrs("coucou", "coucou", "", "t"), Ok("coucou"));
        assert_eq!(
            merge_attrs("coucou", "hello", "", "t"),
            Err("'t' has conflicting definitions: '\"coucou\"', '\"hello\"'".into())
        );

        assert_eq!(merge_attrs(0, 0, 0, "t"), Ok(0));
        assert_eq!(merge_attrs(5, 0, 0, "t"), Ok(5));
        assert_eq!(merge_attrs(0, 5, 0, "t"), Ok(5));
        assert_eq!(merge_attrs(5, 5, 0, "t"), Ok(5));
        assert_eq!(
            merge_attrs(5, 6, 0, "t"),
            Err("'t' has conflicting definitions: '5', '6'".into())
        );
    }
}
