//! This module provides [`parse_duration`], which parses duration in reaction's format (ie. `6h`, `3 days`)
//!
//! Like in those reaction core settings:
//! - [Filters' `retryperiod`](https://reaction.ppom.me/reference.html#retryperiod)
//! - [Actions' `after`](https://reaction.ppom.me/reference.html#after).

use std::time::Duration;

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

#[cfg(test)]
mod tests {

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
}
