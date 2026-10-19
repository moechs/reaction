use std::cmp::Ordering;

use regex::{Captures, Regex, RegexSet};
use serde::{Deserialize, Serialize};

mod ip;

pub use ip::{PatternIp, PatternType};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(Default))]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    #[serde(default)]
    pub regex: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<String>,

    #[serde(default, rename = "ignoreregex", skip_serializing_if = "Vec::is_empty")]
    pub ignore_regex: Vec<String>,
    #[serde(skip)]
    pub compiled_ignore_regex: RegexSet,

    #[serde(flatten)]
    pub ip: PatternIp,

    #[serde(skip)]
    pub name: String,
    #[serde(skip)]
    pub name_with_braces: String,
}

impl Pattern {
    #[cfg(test)]
    pub fn from_name(name: &str) -> Pattern {
        Pattern {
            name: name.into(),
            ..Pattern::default()
        }
    }

    pub fn name_with_braces(&self) -> &String {
        &self.name_with_braces
    }

    pub fn pattern_type(&self) -> PatternType {
        self.ip.pattern_type
    }

    pub fn setup(&mut self, name: &str) -> Result<(), String> {
        self._setup(name)
            .map_err(|msg| format!("pattern {}: {}", name, msg))
    }
    fn _setup(&mut self, name: &str) -> Result<(), String> {
        self.name = name.to_string();
        self.name_with_braces = format!("<{}>", name);

        if self.name.is_empty() {
            return Err("pattern name is empty".into());
        }
        if self.name.contains('.') {
            return Err("character '.' is not allowed in pattern name".into());
        }

        if let Some(regex) = self.ip.setup()? {
            if !self.regex.is_empty() {
                return Err("patterns of type ip, ipv4, ipv6 have a built-in regex defined. you should not define it yourself".into());
            }
            self.regex = regex;
        }

        if self.regex.is_empty() {
            return Err("regex is empty".into());
        }
        let compiled = self.compiled()?;

        self.regex = format!("(?P<{}>{})", self.name, self.regex);

        for ignore in &self.ignore {
            if !compiled.is_match(ignore) {
                return Err(format!(
                    "ignore '{}' doesn't match pattern. It should be fixed or removed.",
                    ignore,
                ));
            }
        }

        self.compiled_ignore_regex =
            match RegexSet::new(self.ignore_regex.iter().map(|regex| format!("^{}$", regex))) {
                Ok(set) => set,
                Err(err) => {
                    // Recompile regexes one by one to display a more specific error
                    for ignore_regex in &self.ignore_regex {
                        Regex::new(&format!("^{}$", ignore_regex))
                            .map_err(|err| format!("ignoreregex '{}': {}", ignore_regex, err))?;
                    }
                    // Here we should have returned an error already.
                    // Returning a more generic error if not (which shouldn't happen).
                    return Err(format!("ignoreregex: {}", err));
                }
            };
        self.ignore_regex = Vec::default();

        Ok(())
    }

    /// Returns the pattern's regex compiled standalone, enclosed in ^ and $
    /// It's not kept as a field of the [`Pattern`] struct
    /// because it's only used during setup and for the `trigger` manual command.
    ///
    /// *Yes, I know, avoiding a few bytes of memory is certainly a bad idea.*
    /// *I'm open to discussion.*
    fn compiled(&self) -> Result<Regex, String> {
        Regex::new(&format!("^{}$", self.regex)).map_err(|err| err.to_string())
    }

    /// Normalize the pattern.
    /// This should happen after checking on ignores.
    /// No-op when the pattern is not an IP.
    /// Otherwise BitAnd the IP with its configured mask,
    /// and add the /<mask>
    pub fn normalize(&self, match_: &mut String) {
        self.ip.normalize(match_)
    }

    /// Whether the provided string is a match for this pattern or not.
    ///
    /// Doesn't take into account ignore and ignore_regex:
    /// use [`Self::is_ignore`] to access this information.
    pub fn is_match(&self, match_: &str) -> bool {
        match self.compiled() {
            Ok(regex) => regex.is_match(match_),
            // Should not happen, this function should be called only after
            // [`Pattern::setup`]
            Err(_) => false,
        }
    }

    /// Whether the provided string is ignored by the ignore or ignoreregex
    /// fields of this pattern.
    ///
    /// Can be used in combination with [`Self::is_match`].
    pub fn is_ignore(&self, match_: &str) -> bool {
        self.ignore.iter().any(|ignore| ignore == match_)
            || self.compiled_ignore_regex.is_match(match_)
            || self.ip.is_ignore(match_)
    }

    /// compiles is_match, is_ignore and normalize
    pub fn get_match(&self, captures: &Captures<'_>, use_ignore: bool) -> Option<String> {
        // if the pattern is in an optional part of the regex,
        // there may be no captured group for it.
        if let Some(match_) = captures.name(&self.name)
            && (!use_ignore || !self.is_ignore(match_.as_str()))
        {
            let mut match_ = match_.as_str().to_string();
            self.normalize(&mut match_);
            Some(match_)
        } else {
            None
        }
    }
}

// This is required to be added to a BTreeSet
// We compare Patterns by their names, which are unique.
// This is enforced by Patterns' names coming from their keys in a BTreeMap in Config
impl Ord for Pattern {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}
impl PartialOrd for Pattern {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Eq for Pattern {}
impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

#[cfg(test)]
impl Pattern {
    /// Test-only constructor designed to be easy to call
    pub fn new(name: &str, regex: &str) -> Result<Self, String> {
        let mut pattern = Self {
            regex: regex.into(),
            ..Default::default()
        };
        pattern.setup(name)?;
        Ok(pattern)
    }

    /// Test-only constructor designed to be easy to call.
    /// Constructs a full [`super::Patterns`] collection with one given pattern
    pub fn new_map(name: &str, regex: &str) -> Result<super::Patterns, String> {
        Ok(std::iter::once((name.into(), Self::new(name, regex)?.into())).collect())
    }
}

#[cfg(test)]
pub mod tests {

    use super::*;

    pub fn default_pattern() -> Pattern {
        Pattern {
            regex: "".into(),
            ignore: Vec::new(),
            ignore_regex: Vec::new(),
            compiled_ignore_regex: RegexSet::default(),
            ip: PatternIp::default(),
            name: "".into(),
            name_with_braces: "".into(),
        }
    }

    pub fn ok_pattern() -> Pattern {
        let mut pattern = default_pattern();
        pattern.regex = "[abc]".into();
        pattern
    }

    pub fn ok_pattern_with_ignore() -> Pattern {
        let mut pattern = ok_pattern();
        pattern.ignore.push("a".into());
        pattern
    }

    pub fn boubou_pattern_with_ignore() -> Pattern {
        let mut pattern = ok_pattern();
        pattern.regex = "(?:bou){1,3}".to_string();
        pattern.ignore.push("boubou".into());
        pattern
    }

    pub fn number_pattern() -> Pattern {
        let mut pattern = ok_pattern();
        pattern.regex = "[0-1]+".to_string();
        pattern
    }

    #[test]
    fn setup_missing_information() {
        let mut pattern;

        // Empty name
        pattern = default_pattern();
        pattern.regex = "abc".into();
        assert!(pattern.setup("").is_err());

        // '.' in name
        pattern = default_pattern();
        pattern.regex = "abc".into();
        assert!(pattern.setup("na.me").is_err());

        // Empty regex
        pattern = default_pattern();
        assert!(pattern.setup("name").is_err());
    }

    #[test]
    fn setup_regex() {
        let mut pattern;

        // regex ok
        pattern = ok_pattern();
        assert!(pattern.setup("name").is_ok());

        // regex ok
        pattern = default_pattern();
        pattern.regex = "abc".into();
        assert!(pattern.setup("name").is_ok());

        // regex ko
        pattern = default_pattern();
        pattern.regex = "[abc".into();
        assert!(pattern.setup("name").is_err());
    }

    #[test]
    fn setup_ignore() {
        let mut pattern;

        // ignore ok
        pattern = default_pattern();
        pattern.regex = "[abc]".into();
        pattern.ignore.push("a".into());
        pattern.ignore.push("b".into());
        assert!(pattern.setup("name").is_ok());

        // ignore ko
        pattern = default_pattern();
        pattern.regex = "[abc]".into();
        pattern.ignore.push("d".into());
        assert!(pattern.setup("name").is_err());
    }

    #[test]
    fn setup_ignore_regex() {
        let mut pattern;

        // ignore_regex ok
        pattern = default_pattern();
        pattern.regex = "[abc]".into();
        pattern.ignore_regex.push("[a]".into());
        pattern.ignore_regex.push("a".into());
        assert!(pattern.setup("name").is_ok());

        // ignore_regex ko
        pattern = default_pattern();
        pattern.regex = "[abc]".into();
        pattern.ignore.push("[a".into());
        assert!(pattern.setup("name").is_err());
    }

    #[test]
    fn setup_yml() {
        let mut pattern: Pattern = serde_saphyr::from_str("{}").unwrap();
        assert!(pattern.setup("name").is_err());

        let mut pattern: Pattern = serde_saphyr::from_str(r#"regex: "[abc]""#).unwrap();
        assert!(pattern.setup("name").is_ok());

        let mut pattern: Pattern = serde_saphyr::from_str(r#"type: ip"#).unwrap();
        assert!(pattern.setup("name").is_ok());

        let mut pattern: Pattern = serde_saphyr::from_str(r#"type: ipv4"#).unwrap();
        assert!(pattern.setup("name").is_ok());

        let mut pattern: Pattern = serde_saphyr::from_str(r#"type: ipv6"#).unwrap();
        assert!(pattern.setup("name").is_ok());

        assert!(serde_saphyr::from_str::<Pattern>(r#"type: zblorg"#).is_err());
    }

    #[test]
    fn is_ignore() {
        let mut pattern;

        // ignore ok
        pattern = default_pattern();
        pattern.regex = "[abcdefg]".into();
        pattern.ignore.push("a".into());
        pattern.ignore.push("b".into());
        pattern.ignore_regex.push("c".into());
        pattern.ignore_regex.push("[de]".into());

        pattern.setup("name").unwrap();
        assert!(pattern.is_ignore("a"));
        assert!(pattern.is_ignore("b"));
        assert!(pattern.is_ignore("c"));
        assert!(pattern.is_ignore("d"));
        assert!(pattern.is_ignore("e"));
        assert!(!pattern.is_ignore("f"));
        assert!(!pattern.is_ignore("g"));
        assert!(!pattern.is_ignore("h"));
    }
}
