use std::collections::BTreeSet;

use chrono::{Local, TimeZone};
use serde_json::Value;

use crate::concepts::{Match, Time};

/// Tries to convert a [`Value`] into a [`String`]
pub fn to_string(val: &Value) -> Result<String, String> {
    Ok(val.as_str().ok_or("not a string")?.to_owned())
}

/// Tries to convert a [`Value`] into a [`Time`]
pub fn to_time(val: &Value) -> Result<Time, String> {
    Ok(Local
        .timestamp_millis_opt(val.as_i64().ok_or("not a number")?)
        .single()
        .ok_or("not a valid timestamp")?)
}

/// Tries to convert a [`Value`] into a [`Match`]
pub fn to_match(val: &Value) -> Result<Match, String> {
    val.as_array()
        .ok_or("not an array")?
        .into_iter()
        .map(|v| to_string(v))
        .collect()
}

/// Tries to convert a [`Value`] into a [`BTreeSet<Time>`]
pub fn to_timeset(val: &Value) -> Result<BTreeSet<Time>, String> {
    val.as_array()
        .ok_or("not an array")?
        .into_iter()
        .map(|v| to_time(v))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_string() {
        assert_eq!(to_string(&("".into())), Ok("".into()));
        assert_eq!(to_string(&("ploup".into())), Ok("ploup".into()));

        assert!(to_string(&(["ploup"].into())).is_err());
        assert!(to_string(&(true.into())).is_err());
        assert!(to_string(&(8.into())).is_err());
        assert!(to_string(&(None::<String>.into())).is_err());
    }

    #[test]
    fn test_to_time() {
        assert_eq!(
            to_time(&(123456.into())).unwrap(),
            Local.timestamp_millis_opt(123456).unwrap(),
        );
        assert!(to_time(&(u64::MAX.into())).is_err());

        assert!(to_time(&(["ploup"].into())).is_err());
        assert!(to_time(&(true.into())).is_err());
        assert!(to_time(&("12345".into())).is_err());
        assert!(to_time(&(None::<String>.into())).is_err());
    }

    #[test]
    fn test_to_match() {
        assert_eq!(to_match(&([""].into())), Ok(vec!["".into()]));
        assert_eq!(
            to_match(&(["plip", "ploup"].into())),
            Ok(vec!["plip".into(), "ploup".into()])
        );
        assert!(to_match(&[Value::from("plip"), Value::from(10)].into()).is_err());

        assert!(to_match(&("ploup".into())).is_err());
        assert!(to_match(&(true.into())).is_err());
        assert!(to_match(&(8.into())).is_err());
        assert!(to_match(&(None::<String>.into())).is_err());
    }

    #[test]
    fn test_to_timeset() {
        assert_eq!(
            to_timeset(&([1234567].into())),
            Ok(BTreeSet::from([Local
                .timestamp_millis_opt(1234567)
                .unwrap()]))
        );
        assert_eq!(
            to_timeset(&([8, 123456].into())),
            Ok(BTreeSet::from([
                Local.timestamp_millis_opt(8).unwrap(),
                Local.timestamp_millis_opt(123456).unwrap()
            ]))
        );
        assert!(to_timeset(&[Value::from("plip"), Value::from(10)].into()).is_err());

        assert!(to_timeset(&([""].into())).is_err());
        assert!(to_timeset(&(["ploup"].into())).is_err());
        assert!(to_timeset(&(true.into())).is_err());
        assert!(to_timeset(&(8.into())).is_err());
        assert!(to_timeset(&(None::<String>.into())).is_err());
    }
}
