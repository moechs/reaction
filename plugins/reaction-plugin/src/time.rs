//! This module provides [`parse_duration`], which parses duration in reaction's format (ie. `6h`, `3 days`)
//!
//! Like in those reaction core settings:
//! - [Filters' `retryperiod`](https://reaction.ppom.me/reference.html#retryperiod)
//! - [Actions' `after`](https://reaction.ppom.me/reference.html#after).

use std::time::Duration;

use serde::{Deserialize, Deserializer, de};

/// Parses the &str argument as a Duration
/// Returns Ok(Duration) if successful, or Err(String).
///
/// Format is defined as follows: `<integer> <unit>`
/// - whitespace between the integer and unit is optional
/// - integer must be positive (>= 0)
/// - unit can be one of:
///   - `ms` / `millis` / `millisecond` / `milliseconds`
///   - `s` / `sec` / `secs` / `second` / `seconds`
///   - `m` / `min` / `mins` / `minute` / `minutes`
///   - `h` / `hour` / `hours`
///   - `d` / `day` / `days`
pub fn parse_duration(d: &str) -> Result<Duration, String> {
    let d_trimmed = d.trim();
    let chars = d_trimmed.as_bytes();
    let mut value = 0;
    let mut i = 0;
    while i < chars.len() && chars[i].is_ascii_digit() {
        value = value * 10 + (chars[i] - b'0') as u32;
        i += 1;
    }
    if i == 0 {
        return Err(format!("duration '{}' doesn't start with digits", d));
    }
    let ok_as = |func: fn(u64) -> Duration| -> Result<_, String> { Ok(func(value as u64)) };

    match d_trimmed[i..].trim() {
        "ms" | "millis" | "millisecond" | "milliseconds" => ok_as(Duration::from_millis),
        "s" | "sec" | "secs" | "second" | "seconds" => ok_as(Duration::from_secs),
        "m" | "min" | "mins" | "minute" | "minutes" => ok_as(Duration::from_mins),
        "h" | "hour" | "hours" => ok_as(Duration::from_hours),
        "d" | "day" | "days" => ok_as(|d: u64| Duration::from_hours(d * 24)),
        unit => Err(format!(
            "unit {} not recognised. must be one of s/sec/seconds, m/min/minutes, h/hours, d/days",
            unit
        )),
    }
}

pub fn print_duration(val: &Duration) -> String {
    let (u, n) = if val.is_zero() {
        ("s", 0)
    } else if val.as_millis() % 1000 != 0 {
        ("ms", val.as_millis())
    } else if val.as_secs() % (3600 * 24) == 0 {
        ("d", (val.as_secs() / (3600 * 24)) as u128)
    } else if val.as_secs() % 3600 == 0 {
        ("h", (val.as_secs() / 3600) as u128)
    } else {
        ("s", val.as_secs() as u128)
    };
    format!("{n}{u}")
}

pub fn deser_duration<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(parse_duration(&s).map_err(de::Error::custom)?)
}

pub fn ser_duration<S>(val: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&print_duration(val))
}

pub fn deser_opt_duration<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: Deserializer<'de>,
{
    if let Ok(s) = String::deserialize(deserializer) {
        Ok(Some(parse_duration(&s).map_err(de::Error::custom)?))
    } else {
        Ok(None)
    }
}

pub fn ser_opt_duration<S>(val: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if let Some(val) = val {
        ser_duration(val, serializer)
    } else {
        serializer.serialize_none()
    }
}

#[cfg(test)]
mod tests {

    use std::time::Duration;

    use serde::Serialize;

    use super::*;

    #[test]
    fn char_conversion() {
        assert_eq!(b'9' - b'0', 9);
    }

    #[test]
    fn parse_duration_test() {
        assert_eq!(parse_duration("1s"), Ok(Duration::from_secs(1)));
        assert_eq!(parse_duration("12s"), Ok(Duration::from_secs(12)));
        assert_eq!(parse_duration("  12  secs  "), Ok(Duration::from_secs(12)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_mins(2)));
        assert_eq!(parse_duration("6 hours"), Ok(Duration::from_hours(6)));
        assert_eq!(parse_duration("1d"), Ok(Duration::from_hours(1 * 24)));
        assert_eq!(parse_duration("365d"), Ok(Duration::from_hours(365 * 24)));

        assert!(parse_duration("d 3").is_err());
        assert!(parse_duration("d3").is_err());
        assert!(parse_duration("3da").is_err());
        assert!(parse_duration("3_days").is_err());
        assert!(parse_duration("_3d").is_err());
        assert!(parse_duration("3 3d").is_err());
        assert!(parse_duration("3.3d").is_err());
    }

    #[test]
    fn serde_duration() {
        #[derive(Debug, Deserialize, Serialize)]
        struct A {
            #[serde(deserialize_with = "deser_duration", serialize_with = "ser_duration")]
            d: Duration,
        }
        for (s, v) in [
            ("254ms", Duration::from_millis(254)),
            ("12345ms", Duration::from_millis(12345)),
            ("1s", Duration::from_secs(1)),
            ("12s", Duration::from_secs(12)),
            ("3601s", Duration::from_secs(3601)),
            ("6h", Duration::from_hours(6)),
            ("3d", Duration::from_hours(3 * 24)),
        ] {
            let json = format!(r#"{{"d":"{s}"}}"#);
            let parsed = serde_json::from_str::<A>(&json).expect("deser works");
            assert_eq!(v, parsed.d);
            assert_eq!(json, serde_json::to_string(&parsed).expect("ser works"))
        }
    }

    #[test]
    fn serde_option_duration() {
        #[derive(Debug, Deserialize, Serialize)]
        struct A {
            #[serde(
                default,
                skip_serializing_if = "Option::is_none",
                deserialize_with = "deser_opt_duration",
                serialize_with = "ser_opt_duration"
            )]
            d: Option<Duration>,
        }
        for (s, v) in [
            ("254ms", Duration::from_millis(254)),
            ("12345ms", Duration::from_millis(12345)),
            ("1s", Duration::from_secs(1)),
            ("12s", Duration::from_secs(12)),
            ("3601s", Duration::from_secs(3601)),
            ("6h", Duration::from_hours(6)),
            ("3d", Duration::from_hours(3 * 24)),
        ] {
            let json = format!(r#"{{"d":"{s}"}}"#);
            let parsed = serde_json::from_str::<A>(&json).expect("deser works");
            assert_eq!(v, parsed.d.unwrap());
            assert_eq!(json, serde_json::to_string(&parsed).expect("ser works"))
        }
        // None
        let json = r#"{}"#;
        let parsed = serde_json::from_str::<A>(&json).expect("deser works");
        assert_eq!(None, parsed.d);
        assert_eq!(json, serde_json::to_string(&parsed).expect("ser works"))
    }
}
