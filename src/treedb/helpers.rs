use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Local};
use serde_json::Value;

use crate::concepts::{Match, MatchTime, Time};

/// Tries to convert a [`Value`] into a [`String`]
pub fn to_string(val: &Value) -> Result<String, String> {
    Ok(val.as_str().ok_or("not a string")?.to_owned())
}

/// Tries to convert a [`Value`] into a [`u64`]
pub fn to_u64(val: &Value) -> Result<u64, String> {
    val.as_u64().ok_or("not a u64".into())
}

fn string_to_time(val: &str) -> Result<Time, String> {
    Ok(DateTime::parse_from_rfc3339(val)
        .map_err(|err| err.to_string())?
        .with_timezone(&Local))
}

/// Tries to convert a [`Value`] into a [`Time`]
pub fn to_time(val: &Value) -> Result<Time, String> {
    Ok(string_to_time(val.as_str().ok_or("not a number")?)?)
}

/// Tries to convert a [`Value`] into a [`Match`]
pub fn to_match(val: &Value) -> Result<Match, String> {
    val.as_array()
        .ok_or("not an array")?
        .iter()
        .map(to_string)
        .collect()
}

/// Tries to convert a [`Value`] into a [`MatchTime`]
pub fn to_matchtime(val: &Value) -> Result<MatchTime, String> {
    let map = val.as_object().ok_or("not an object")?;
    Ok(MatchTime {
        m: to_match(map.get("m").ok_or("no m in object")?)?,
        t: to_time(map.get("t").ok_or("no t in object")?)?,
    })
}

/// Tries to convert a [`Value`] into a [`BTreeSet<Time>`]
pub fn to_timeset(val: &Value) -> Result<BTreeSet<Time>, String> {
    val.as_array()
        .ok_or("not an array")?
        .iter()
        .map(to_time)
        .collect()
}

/// Tries to convert a [`Value`] into a [`BTreeMap<Time, u64>`]
pub fn to_timemap(val: &Value) -> Result<BTreeMap<Time, u64>, String> {
    val.as_object()
        .ok_or("not a map")?
        .iter()
        .map(|(key, value)| Ok((string_to_time(key)?, to_u64(value)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::TimeZone;
    use serde_json::Map;

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
    fn test_to_u64() {
        assert_eq!(to_u64(&(0.into())), Ok(0));
        assert_eq!(to_u64(&(8.into())), Ok(8));
        assert_eq!(to_u64(&(u64::MAX.into())), Ok(u64::MAX));

        assert!(to_u64(&("ploup".into())).is_err());
        assert!(to_u64(&(["ploup"].into())).is_err());
        assert!(to_u64(&(true.into())).is_err());
        assert!(to_u64(&(None::<String>.into())).is_err());
    }

    #[test]
    fn test_to_time() {
        assert_eq!(
            to_time(&"1970-01-01T01:02:03.456+01:00".into()).unwrap(),
            Local.timestamp_millis_opt(123456).unwrap(),
        );
        assert!(to_time(&(u64::MAX.into())).is_err());

        assert!(to_time(&(["ploup"].into())).is_err());
        assert!(to_time(&(true.into())).is_err());
        assert!(to_time(&(12345.into())).is_err());
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
            to_timeset(&(["1970-01-01T01:20:34.567+01:00"].into())),
            Ok(BTreeSet::from([Local
                .timestamp_millis_opt(1234567)
                .unwrap()]))
        );
        assert_eq!(
            to_timeset(
                &([
                    "1970-01-01T01:00:00.008+01:00",
                    "1970-01-01T01:02:03.456+01:00"
                ]
                .into())
            ),
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

    #[test]
    fn test_to_matchtime() {
        assert_eq!(
            to_matchtime(&Value::Object(Map::from_iter(
                BTreeMap::from([
                    ("m".into(), ["plip", "ploup"].into()),
                    ("t".into(), "1970-01-01T04:25:45.678+01:00".into()),
                ])
                .into_iter()
            ))),
            Ok(MatchTime {
                m: vec!["plip".into(), "ploup".into()],
                t: Local.timestamp_millis_opt(12345678).unwrap(),
            })
        );

        assert!(to_matchtime(&Value::Object(Map::from_iter(
            BTreeMap::from([("m".into(), ["plip", "ploup"].into()),]).into_iter()
        )))
        .is_err());

        assert!(to_matchtime(&Value::Object(Map::from_iter(
            BTreeMap::from([("t".into(), 12345678.into()),]).into_iter()
        )))
        .is_err());

        assert!(to_matchtime(&Value::Object(Map::from_iter(
            BTreeMap::from([("m".into(), "ploup".into()), ("t".into(), 12345678.into()),])
                .into_iter()
        )))
        .is_err());

        assert!(to_matchtime(&Value::Object(Map::from_iter(
            BTreeMap::from([
                ("m".into(), ["plip", "ploup"].into()),
                ("t".into(), [1234567].into()),
            ])
            .into_iter()
        )))
        .is_err());

        assert!(to_timeset(&([""].into())).is_err());
        assert!(to_timeset(&(["ploup"].into())).is_err());
        assert!(to_timeset(&(true.into())).is_err());
        assert!(to_timeset(&(8.into())).is_err());
        assert!(to_timeset(&(None::<String>.into())).is_err());
    }
}
