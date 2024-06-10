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
    pub name_with_braces: String,
}

impl Pattern {
    pub fn setup(&mut self, name: &String) -> Result<(), String> {
        self._setup(name)
            .or_else(|msg| Err(format!("pattern {}: {}", name, msg)))
    }

    pub fn _setup(&mut self, name: &String) -> Result<(), String> {
        self.name = name.clone();
        self.name_with_braces = format!("<{}>", name);

        if self.regex.len() == 0 {
            return Err("regex is empty".into());
        }
        let compiled =
            Regex::new(&format!("^{}$", self.regex)).or_else(|err| Err(err.to_string()))?;

        self.regex = format!("(?P<{}>{})", self.name, self.regex);

        for ignore in &self.ignore {
            if !compiled.is_match(&ignore) {
                return Err(format!(
                    "ignore '{}' doesn't match pattern. It should be fixed or removed.",
                    ignore,
                ));
            }
        }

        for ignore_regex in &self.ignore_regex {
            let compiled_ignore = Regex::new(&format!("^{}$", ignore_regex)).or_else(|err| {
                Err(format!(
                    "ignoreregex '{}': {}",
                    ignore_regex,
                    err.to_string()
                ))
            })?;

            self.compiled_ignore_regex.push(compiled_ignore);
        }

        Ok(())
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
