use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

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

pub fn string_to_time(val: &str) -> Result<Time, String> {
    let nanos: u128 = val.parse().map_err(|_| "not a number")?;
    Ok(Duration::new(
        (nanos / 1_000_000_000) as u64,
        (nanos % 1_000_000_000) as u32,
    )
    .into())
}

/// Tries to convert a [`Value`] into a [`Time`]
pub fn to_time(val: &Value) -> Result<Time, String> {
    string_to_time(val.as_str().ok_or("not a string number")?)
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
        assert_eq!(to_time(&"123456".into()).unwrap(), Time::from_nanos(123456),);
        assert!(to_time(&(u64::MAX.into())).is_err());

        assert!(to_time(&(["ploup"].into())).is_err());
        assert!(to_time(&(true.into())).is_err());
        // assert!(to_time(&(12345.into())).is_err());
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
            to_timeset(&Value::from([Value::from("123456789")])),
            Ok(BTreeSet::from([Time::from_nanos(123456789)]))
        );
        assert_eq!(
            to_timeset(&Value::from([Value::from("8"), Value::from("123456")])),
            Ok(BTreeSet::from([
                Time::from_nanos(8),
                Time::from_nanos(123456),
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
                    ("t".into(), "12345678".into()),
                ])
                .into_iter()
            ))),
            Ok(MatchTime {
                m: vec!["plip".into(), "ploup".into()],
                t: Time::from_nanos(12345678),
            })
        );

        assert!(
            to_matchtime(&Value::Object(Map::from_iter(
                BTreeMap::from([("m".into(), ["plip", "ploup"].into()),]).into_iter()
            )))
            .is_err()
        );

        assert!(
            to_matchtime(&Value::Object(Map::from_iter(
                BTreeMap::from([("t".into(), 12345678.into()),]).into_iter()
            )))
            .is_err()
        );

        assert!(
            to_matchtime(&Value::Object(Map::from_iter(
                BTreeMap::from([("m".into(), "ploup".into()), ("t".into(), 12345678.into()),])
                    .into_iter()
            )))
            .is_err()
        );

        assert!(
            to_matchtime(&Value::Object(Map::from_iter(
                BTreeMap::from([
                    ("m".into(), ["plip", "ploup"].into()),
                    ("t".into(), [1234567].into()),
                ])
                .into_iter()
            )))
            .is_err()
        );

        assert!(to_timeset(&([""].into())).is_err());
        assert!(to_timeset(&(["ploup"].into())).is_err());
        assert!(to_timeset(&(true.into())).is_err());
        assert!(to_timeset(&(8.into())).is_err());
        assert!(to_timeset(&(None::<String>.into())).is_err());
    }

    #[test]
    fn test_to_timemap() {
        let time1 = 1234567;
        let time1_t = Time::from_nanos(time1);
        let time2 = 123456789;
        let time2_t = Time::from_nanos(time2);

        assert_eq!(
            to_timemap(&Value::from_iter([(time2.to_string(), 1)])),
            Ok(BTreeMap::from([(time2_t, 1)]))
        );
        assert_eq!(
            to_timemap(&Value::from_iter([
                (time1.to_string(), 4),
                (time2.to_string(), 0)
            ])),
            Ok(BTreeMap::from([(time1_t.into(), 4), (time2_t.into(), 0)]))
        );

        assert!(to_timemap(&Value::from_iter([("1-1", time2)])).is_err());
        // assert!(to_timemap(&Value::from_iter([(time2.to_string(), time2)])).is_err());
        assert!(to_timemap(&Value::from_iter([(time2)])).is_err());
        assert!(to_timemap(&Value::from_iter([(1)])).is_err());

        assert!(to_timemap(&(["1970-01-01T01:20:34.567+01:00"].into())).is_err());
        assert!(to_timemap(&([""].into())).is_err());
        assert!(to_timemap(&(["ploup"].into())).is_err());
        assert!(to_timemap(&(true.into())).is_err());
        assert!(to_timemap(&(8.into())).is_err());
        assert!(to_timemap(&(None::<String>.into())).is_err());
    }
}
