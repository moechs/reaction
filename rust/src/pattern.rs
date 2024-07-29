use std::cmp::Ordering;

use regex::Regex;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub regex: String,

    #[serde(default)]
    ignore: Vec<String>,

    #[serde(default, rename = "ignoreregex")]
    ignore_regex: Vec<String>,
    #[serde(skip)]
    compiled_ignore_regex: Vec<Regex>,

    #[serde(skip)]
    name: String,
    #[serde(skip)]
    name_with_braces: String,
}

impl Pattern {
    pub fn setup(&mut self, name: &String) -> Result<(), String> {
        self._setup(name)
            .map_err(|msg| format!("pattern {}: {}", name, msg))
    }

    pub fn name(&self) -> &String {
        &self.name
    }
    pub fn name_with_braces(&self) -> &String {
        &self.name_with_braces
    }

    pub fn _setup(&mut self, name: &String) -> Result<(), String> {
        self.name = name.clone();
        self.name_with_braces = format!("<{}>", name);

        if self.name.is_empty() {
            return Err("pattern name is empty".into());
        }
        if self.name.contains('.') {
            return Err("character '.' is not allowed in pattern name".into());
        }

        if self.regex.is_empty() {
            return Err("regex is empty".into());
        }
        let compiled = Regex::new(&format!("^{}$", self.regex)).map_err(|err| err.to_string())?;

        self.regex = format!("(?P<{}>{})", self.name, self.regex);

        for ignore in &self.ignore {
            if !compiled.is_match(ignore) {
                return Err(format!(
                    "ignore '{}' doesn't match pattern. It should be fixed or removed.",
                    ignore,
                ));
            }
        }

        for ignore_regex in &self.ignore_regex {
            let compiled_ignore = Regex::new(&format!("^{}$", ignore_regex))
                .map_err(|err| format!("ignoreregex '{}': {}", ignore_regex, err))?;

            self.compiled_ignore_regex.push(compiled_ignore);
        }
        self.ignore_regex.clear();

        Ok(())
    }

    pub fn not_an_ignore(&self, match_: &str) -> bool {
        for regex in &self.compiled_ignore_regex {
            if regex.is_match(match_) {
                return false;
            }
        }
        for ignore in &self.ignore {
            if ignore == match_ {
                return false;
            }
        }
        true
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
pub mod tests {

    use super::*;

    pub fn default_pattern() -> Pattern {
        Pattern {
            regex: "".into(),
            ignore: Vec::new(),
            ignore_regex: Vec::new(),
            compiled_ignore_regex: Vec::new(),
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

    #[test]
    fn setup_missing_information() {
        let mut pattern;

        // Empty name
        pattern = default_pattern();
        pattern.regex = "abc".into();
        assert!(pattern.setup(&"".into()).is_err());

        // '.' in name
        pattern = default_pattern();
        pattern.regex = "abc".into();
        assert!(pattern.setup(&"na.me".into()).is_err());

        // Empty regex
        pattern = default_pattern();
        assert!(pattern.setup(&"name".into()).is_err());
    }

    #[test]
    fn setup_regex() {
        let mut pattern;

        // regex ok
        pattern = ok_pattern();
        assert!(pattern.setup(&"name".into()).is_ok());

        // regex ok
        pattern = default_pattern();
        pattern.regex = "abc".into();
        assert!(pattern.setup(&"name".into()).is_ok());

        // regex ko
        pattern = default_pattern();
        pattern.regex = "[abc".into();
        assert!(pattern.setup(&"name".into()).is_err());
    }

    #[test]
    fn setup_ignore() {
        let mut pattern;

        // ignore ok
        pattern = default_pattern();
        pattern.regex = "[abc]".into();
        pattern.ignore.push("a".into());
        pattern.ignore.push("b".into());
        assert!(pattern.setup(&"name".into()).is_ok());

        // ignore ko
        pattern = default_pattern();
        pattern.regex = "[abc]".into();
        pattern.ignore.push("d".into());
        assert!(pattern.setup(&"name".into()).is_err());
    }

    #[test]
    fn setup_ignore_regex() {
        let mut pattern;

        // ignore_regex ok
        pattern = default_pattern();
        pattern.regex = "[abc]".into();
        pattern.ignore_regex.push("[a]".into());
        pattern.ignore_regex.push("a".into());
        assert!(pattern.setup(&"name".into()).is_ok());

        // ignore_regex ko
        pattern = default_pattern();
        pattern.regex = "[abc]".into();
        pattern.ignore.push("[a".into());
        assert!(pattern.setup(&"name".into()).is_err());
    }

    #[test]
    fn not_an_ignore() {
        let mut pattern;

        // ignore ok
        pattern = default_pattern();
        pattern.regex = "[abcdefg]".into();
        pattern.ignore.push("a".into());
        pattern.ignore.push("b".into());
        pattern.ignore_regex.push("c".into());
        pattern.ignore_regex.push("[de]".into());

        pattern.setup(&"name".into()).unwrap();
        assert!(!pattern.not_an_ignore("a"));
        assert!(!pattern.not_an_ignore("b"));
        assert!(!pattern.not_an_ignore("c"));
        assert!(!pattern.not_an_ignore("d"));
        assert!(!pattern.not_an_ignore("e"));
        assert!(pattern.not_an_ignore("f"));
        assert!(pattern.not_an_ignore("g"));
        assert!(pattern.not_an_ignore("h"));
    }
}
