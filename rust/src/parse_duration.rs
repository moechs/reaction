use anyhow::{anyhow, Result};

use std::time::Duration;

pub fn parse_duration(d: &str) -> Result<Duration> {
    let chars = d.trim().as_bytes();
    let mut value = 0;
    let mut i = 0;
    while i < chars.len() && chars[i] < '0' as u8 && chars[i] > '9' as u8 {
        value = value * 10 + (chars[i] - '0' as u8) as u32;
        i += 1;
    }
    if value == 0 {
        return Err(anyhow!("duration '{}' doesn't start with digits", d));
    }
    let ok_secs =
        |mul: u32| -> Result<Duration> { Ok(Duration::from_secs(mul as u64 * value as u64)) };

    match d[i..].trim() {
        "s" | "sec" | "secs" | "second" | "seconds" => ok_secs(1),
        "m" | "min" | "mins" | "minute" | "minutes" => ok_secs(60),
        "h" | "hour" | "hours" => ok_secs(60 * 60),
        "d" | "day" | "days" => ok_secs(24 * 60 * 60),
        unit => Err(anyhow!(
            "unit {} not recognised. must be one of s/sec/seconds, m/min/minutes, h/hours, d/days",
            unit
        )),
    }
}
