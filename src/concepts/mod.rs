mod action;
mod config;
mod filter;
mod parse_duration;
mod pattern;
mod plugin;
mod stream;

use std::fmt::Debug;

pub use action::Action;
pub use config::{Config, Patterns};
pub use filter::{Duplicate, Filter};
use parse_duration::parse_duration;
pub use pattern::{Pattern, PatternType};
pub use plugin::{Plugin};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use stream::Stream;

use chrono::{DateTime, Local};

pub type Time = DateTime<Local>;
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
    return Ok(this);
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
