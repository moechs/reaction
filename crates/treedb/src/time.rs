use std::{
    fmt,
    ops::{Add, Deref, Sub},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::TimeZone as _;
use serde::{Deserialize, Serialize};

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
impl From<Time> for Duration {
    fn from(value: Time) -> Self {
        value.0
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
        Time(Duration::from_secs(hours * 3600))
    }
    pub fn from_mins(mins: u64) -> Time {
        Time(Duration::from_secs(mins * 60))
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

    pub fn pretty(&self) -> String {
        #[allow(clippy::unwrap_used)]
        chrono::Local
            .timestamp_opt(self.as_secs() as i64, self.subsec_nanos())
            .unwrap()
            .to_rfc3339()
            .chars()
            .take(19)
            .collect()
    }
}

pub fn now() -> Time {
    Time(SystemTime::now().duration_since(UNIX_EPOCH).unwrap())
}
