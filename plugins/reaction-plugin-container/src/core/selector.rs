//! Container selection rules.
//!
//! Semantics (shared by `match` and `exclude`):
//! - the dimensions (`names`, `labels`) are OR-ed: matching any dimension is enough;
//! - `names`: the container name equals any listed name;
//! - `labels`: the container carries **all** listed labels (Docker's own label filter semantics).
//!   `key=value` requires that exact value, a bare `key` only requires the key to exist.
//!
//! A stream follows a container when it matches `match` and does not match `exclude`.

use serde::{Deserialize, Deserializer};

use super::meta::ContainerMeta;

/// One `key` or `key=value` label condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelMatcher {
    pub key: String,
    pub value: Option<String>,
}

impl LabelMatcher {
    pub fn parse(s: &str) -> Result<Self, String> {
        let (key, value) = match s.split_once('=') {
            Some((key, value)) => (key, Some(value.to_string())),
            None => (s, None),
        };
        if key.is_empty() {
            return Err(format!("invalid label selector {s:?}: empty key"));
        }
        Ok(Self {
            key: key.to_string(),
            value,
        })
    }

    fn matches(&self, meta: &ContainerMeta) -> bool {
        match (meta.labels.get(&self.key), &self.value) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(actual), Some(expected)) => actual == expected,
        }
    }
}

impl<'de> Deserialize<'de> for LabelMatcher {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        LabelMatcher::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub labels: Vec<LabelMatcher>,
}

impl Selector {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.labels.is_empty()
    }

    pub fn validate(&self, what: &str) -> Result<(), String> {
        if self.names.iter().any(String::is_empty) {
            return Err(format!("{what}.names contains an empty name"));
        }
        Ok(())
    }

    pub fn matches(&self, meta: &ContainerMeta) -> bool {
        let by_name = self.names.contains(&meta.name);
        let by_labels = !self.labels.is_empty() && self.labels.iter().all(|l| l.matches(meta));
        by_name || by_labels
    }
}

/// `match` minus `exclude`.
#[derive(Debug, Clone)]
pub struct Filter {
    pub include: Selector,
    pub exclude: Selector,
}

impl Filter {
    pub fn matches(&self, meta: &ContainerMeta) -> bool {
        self.include.matches(meta) && !self.exclude.matches(meta)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn meta(name: &str, labels: &[(&str, &str)]) -> ContainerMeta {
        ContainerMeta {
            id: "0123456789abcdef".into(),
            name: name.into(),
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
            ..Default::default()
        }
    }

    fn sel(names: &[&str], labels: &[&str]) -> Selector {
        Selector {
            names: names.iter().map(|s| s.to_string()).collect(),
            labels: labels
                .iter()
                .map(|s| LabelMatcher::parse(s).unwrap())
                .collect(),
        }
    }

    #[test]
    fn label_parse() {
        assert_eq!(
            LabelMatcher::parse("a=b=c").unwrap(),
            LabelMatcher {
                key: "a".into(),
                value: Some("b=c".into())
            }
        );
        assert_eq!(
            LabelMatcher::parse("a=").unwrap(),
            LabelMatcher {
                key: "a".into(),
                value: Some("".into())
            }
        );
        assert_eq!(LabelMatcher::parse("a").unwrap().value, None);
        assert!(LabelMatcher::parse("=b").is_err());
        assert!(LabelMatcher::parse("").is_err());
    }

    #[test]
    fn names_or_labels() {
        let s = sel(&["web"], &["app=shop", "tier"]);
        assert!(s.matches(&meta("web", &[])));
        assert!(s.matches(&meta("api", &[("app", "shop"), ("tier", "front")])));
        // labels are AND-ed
        assert!(!s.matches(&meta("api", &[("app", "shop")])));
        assert!(!s.matches(&meta("api", &[("app", "other"), ("tier", "x")])));
        // names are exact
        assert!(!s.matches(&meta("web-1", &[])));
    }

    #[test]
    fn empty_selector_matches_nothing() {
        let s = Selector::default();
        assert!(s.is_empty());
        assert!(!s.matches(&meta("web", &[("a", "b")])));
    }

    #[test]
    fn exclude_wins() {
        let f = Filter {
            include: sel(&[], &["app=shop"]),
            exclude: sel(&["shop-db"], &["reaction.ignore"]),
        };
        assert!(f.matches(&meta("shop-web", &[("app", "shop")])));
        assert!(!f.matches(&meta("shop-db", &[("app", "shop")])));
        assert!(!f.matches(&meta(
            "shop-cache",
            &[("app", "shop"), ("reaction.ignore", "")]
        )));
    }
}
