use std::time::Duration;

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
    let ok_secs = |mul: u32| -> Result<Duration, String> {
        Ok(Duration::from_secs(mul as u64 * value as u64))
    };

    match d_trimmed[i..].trim() {
        "s" | "sec" | "secs" | "second" | "seconds" => ok_secs(1),
        "m" | "min" | "mins" | "minute" | "minutes" => ok_secs(60),
        "h" | "hour" | "hours" => ok_secs(60 * 60),
        "d" | "day" | "days" => ok_secs(24 * 60 * 60),
        unit => Err(format!(
            "unit {} not recognised. must be one of s/sec/seconds, m/min/minutes, h/hours, d/days",
            unit
        )),
    }
}

#[cfg(test)]
mod tests {

    use std::time::Duration;

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
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(2 * 60)));
        assert_eq!(
            parse_duration("6 hours"),
            Ok(Duration::from_secs(6 * 60 * 60))
        );
        assert_eq!(parse_duration("1d"), Ok(Duration::from_secs(24 * 60 * 60)));
        assert_eq!(
            parse_duration("365d"),
            Ok(Duration::from_secs(365 * 24 * 60 * 60))
        );

        assert!(parse_duration("d 3").is_err());
        assert!(parse_duration("d3").is_err());
        assert!(parse_duration("3da").is_err());
        assert!(parse_duration("3_days").is_err());
        assert!(parse_duration("_3d").is_err());
        assert!(parse_duration("3 3d").is_err());
        assert!(parse_duration("3.3d").is_err());
    }
}
