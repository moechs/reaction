use std::sync::Arc;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::concepts::{LineContext, Match, Patterns, filter::compute_regex_with_patterns};

use super::{FilterMethodAs, PatternSet};

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct FilterMethodRegex {
    pub regex: Vec<String>,
    #[serde(skip)]
    pub compiled_regex: Vec<Regex>,
    #[serde(skip)]
    pub patterns: Arc<PatternSet>,
}

impl FilterMethodAs for FilterMethodRegex {
    fn setup(&mut self, config_patterns: &Patterns) -> Result<Arc<PatternSet>, String> {
        let mut new_patterns = None;

        for regex in std::mem::take(&mut self.regex) {
            let (regex_buf, compiled, new_patterns_regex) =
                compute_regex_with_patterns(regex, config_patterns)?;

            if let Some(new_patterns) = &new_patterns {
                if let Some(pattern) = new_patterns_regex.symmetric_difference(new_patterns).next()
                {
                    return Err(format!(
                        "pattern {} is present in a regex but is not present in another regex. all regexes should contain the same set of patterns",
                        &pattern.name_with_braces()
                    ));
                }
            } else {
                new_patterns = Some(new_patterns_regex);
            }

            self.regex.push(regex_buf);
            self.compiled_regex.push(compiled);
        }

        // Here we check that we have at least one regex
        self.patterns = Arc::new(new_patterns.ok_or("no regex configured")?);

        Ok(self.patterns.clone())
    }

    fn get_match<'a>(
        &self,
        line: &'a str,
        _context: &mut LineContext<'a>,
        use_ignore: bool,
    ) -> Result<Option<Match>, String> {
        for regex in &self.compiled_regex {
            if let Some(matches) = regex.captures(line) {
                if !self.patterns.is_empty() {
                    let result: Match = self
                        .patterns
                        .iter()
                        .map(|pattern| pattern.get_match(&matches, use_ignore))
                        .filter(Option::is_some)
                        .map(Option::unwrap)
                        .collect();
                    if result.len() == self.patterns.len() {
                        return Ok(Some(result));
                    }
                } else {
                    return Ok(Some(vec![]));
                }
            }
        }
        Ok(None)
    }

    fn hint_regexes(&self) -> Option<Vec<String>> {
        Some(self.regex.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use regex::Regex;

    use crate::concepts::{
        LineContext, Patterns,
        filter::{FilterMethodAs, as_regex::FilterMethodRegex},
        pattern::tests::{boubou_pattern_with_ignore, default_pattern, ok_pattern_with_ignore},
    };

    /// Log lines from applications with ANSI colors enabled (e.g.
    /// tracing_subscriber) contain escape sequences between field names,
    /// `=` and values. These break literal-text regexes such as
    /// `client_ip="<ip>"`.
    #[test]
    fn ansi_escape_codes_prevent_literal_match() {
        use crate::concepts::pattern::{Pattern, PatternIp, PatternType};

        let ip_name = "ip".to_string();
        let mut ip_pattern = Pattern {
            ip: PatternIp {
                pattern_type: PatternType::Ip,
                ..Default::default()
            },
            ..Default::default()
        };
        ip_pattern.setup(&ip_name).unwrap();
        let patterns = Patterns::from([(ip_name, Arc::new(ip_pattern))]);

        let mut filter = FilterMethodRegex::default();
        filter.regex.push(r#"client_ip="<ip>""#.to_string());
        filter.setup(&patterns).unwrap();

        // Plain log line (what the user copied into test-regex) matches.
        let plain = r#"lorica: request completed client_ip="127.0.0.1" status=403"#;
        assert_eq!(
            filter
                .get_match(plain, &mut LineContext::default(), true)
                .unwrap(),
            Some(vec!["127.0.0.1".to_string()])
        );

        // Same line as delivered by Docker with ANSI colors enabled: escape
        // codes are inserted between "client_ip", "=" and the value.
        let ansi = "lorica: request completed \x1b[3mclient_ip\x1b[0m\x1b[2m=\x1b[0m\"127.0.0.1\" \x1b[3mstatus\x1b[0m\x1b[2m=\x1b[0m403";
        assert_eq!(
            filter
                .get_match(ansi, &mut LineContext::default(), true)
                .unwrap(),
            None
        );
    }

    // TODO move things here
    #[test]
    fn setup_regexes() {
        let name = "name".to_string();
        let mut method_regex;

        // make a Patterns
        let mut patterns = Patterns::new();

        let mut pattern = default_pattern();
        pattern.regex = "[abc]".to_string();
        assert!(pattern.setup(&name).is_ok());
        patterns.insert(name.clone(), pattern.clone().into());

        let unused_name = "unused".to_string();
        let mut unused_pattern = default_pattern();
        unused_pattern.regex = "compile[error".to_string();
        assert!(unused_pattern.setup(&unused_name).is_err());
        patterns.insert(unused_name.clone(), unused_pattern.clone().into());

        let boubou_name = "boubou".to_string();
        let mut boubou = boubou_pattern_with_ignore();
        boubou.setup(&boubou_name).unwrap();
        patterns.insert(boubou_name.clone(), boubou.clone().into());

        // correct regex replacement
        method_regex = FilterMethodRegex::default();
        method_regex.regex.push("insert <name> here$".to_string());
        assert!(method_regex.setup(&patterns).is_ok());
        assert_eq!(
            method_regex.compiled_regex[0].to_string(),
            Regex::new("insert (?P<name>[abc]) here$")
                .unwrap()
                .to_string()
        );
        assert_eq!(
            &method_regex.regex[0].to_string(),
            "insert (?P<name>[abc]) here$"
        );
        assert_eq!(method_regex.patterns.len(), 1);
        let stored_pattern = method_regex.patterns.first().unwrap();
        assert_eq!(stored_pattern.regex, pattern.regex);

        // same pattern two times in regex
        method_regex = FilterMethodRegex::default();
        method_regex
            .regex
            .push("there <name> are two <name> s!".to_string());
        assert!(method_regex.setup(&patterns).is_err());

        // two patterns in one regex
        method_regex = FilterMethodRegex::default();
        method_regex
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        assert!(method_regex.setup(&patterns).is_ok());
        assert_eq!(
            method_regex.compiled_regex[0].to_string(),
            Regex::new("insert (?P<name>[abc]) here and (?P<boubou>(?:bou){1,3}) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(
            &method_regex.compiled_regex[0].to_string(),
            "insert (?P<name>[abc]) here and (?P<boubou>(?:bou){1,3}) there"
        );
        assert_eq!(method_regex.patterns.len(), 2);
        let stored_pattern = method_regex.patterns.first().unwrap();
        assert_eq!(stored_pattern.regex, boubou.regex);
        let stored_pattern = method_regex.patterns.last().unwrap();
        assert_eq!(stored_pattern.regex, pattern.regex);

        // multiple regexes with same pattern
        method_regex = FilterMethodRegex::default();
        method_regex.regex.push("insert <name> here".to_string());
        method_regex.regex.push("also add <name> there".to_string());
        assert!(method_regex.setup(&patterns).is_ok());
        assert_eq!(
            method_regex.compiled_regex[0].to_string(),
            Regex::new("insert (?P<name>[abc]) here")
                .unwrap()
                .to_string()
        );
        assert_eq!(
            &method_regex.compiled_regex[0].to_string(),
            "insert (?P<name>[abc]) here"
        );
        assert_eq!(
            method_regex.compiled_regex[1].to_string(),
            Regex::new("also add (?P<name>[abc]) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(
            &method_regex.compiled_regex[1].to_string(),
            "also add (?P<name>[abc]) there"
        );
        assert_eq!(method_regex.patterns.len(), 1);
        let stored_pattern = method_regex.patterns.first().unwrap();
        assert_eq!(stored_pattern.regex, pattern.regex);

        // multiple regexes with same patterns
        method_regex = FilterMethodRegex::default();
        method_regex
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        method_regex
            .regex
            .push("also add <boubou> here and <name> there".to_string());
        assert!(method_regex.setup(&patterns).is_ok());
        assert_eq!(
            method_regex.compiled_regex[0].to_string(),
            Regex::new("insert (?P<name>[abc]) here and (?P<boubou>(?:bou){1,3}) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(
            method_regex.compiled_regex[1].to_string(),
            Regex::new("also add (?P<boubou>(?:bou){1,3}) here and (?P<name>[abc]) there")
                .unwrap()
                .to_string()
        );
        assert_eq!(
            &method_regex.compiled_regex[1].to_string(),
            "also add (?P<boubou>(?:bou){1,3}) here and (?P<name>[abc]) there"
        );
        assert_eq!(method_regex.patterns.len(), 2);
        let stored_pattern = method_regex.patterns.first().unwrap();
        assert_eq!(stored_pattern.regex, boubou.regex);
        let stored_pattern = method_regex.patterns.last().unwrap();
        assert_eq!(stored_pattern.regex, pattern.regex);

        // multiple regexes with different patterns 1
        method_regex = FilterMethodRegex::default();
        method_regex.regex.push("insert <name> here".to_string());
        method_regex
            .regex
            .push("also add <boubou> there".to_string());
        assert!(method_regex.setup(&patterns).is_err());

        // multiple regexes with different patterns 2
        method_regex = FilterMethodRegex::default();
        method_regex
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        method_regex
            .regex
            .push("also add <boubou> there".to_string());
        assert!(method_regex.setup(&patterns).is_err());

        // multiple regexes with different patterns 3
        method_regex = FilterMethodRegex::default();
        method_regex
            .regex
            .push("also add <boubou> there".to_string());
        method_regex
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        assert!(method_regex.setup(&patterns).is_err());
    }

    #[test]
    fn get_match() {
        let name = "name".to_string();
        let mut filter;

        let mut pattern = ok_pattern_with_ignore();
        pattern.setup(&name).unwrap();
        let pattern = Arc::new(pattern);

        let boubou_name = "boubou".to_string();
        let mut boubou = boubou_pattern_with_ignore();
        boubou.setup(&boubou_name).unwrap();
        let boubou = Arc::new(boubou);

        let patterns = Patterns::from([
            (name.clone(), pattern.clone()),
            (boubou_name.clone(), boubou.clone()),
        ]);

        // one simple regex
        filter = FilterMethodRegex::default();
        filter.regex.push("insert <name> here$".to_string());
        assert!(filter.setup(&patterns).is_ok());
        assert_eq!(
            filter
                .get_match("insert b here", &mut LineContext::default(), true)
                .unwrap(),
            Some(vec!("b".into()))
        );
        assert_eq!(
            filter
                .get_match("insert a here", &mut LineContext::default(), true)
                .unwrap(),
            None
        );
        assert_eq!(
            filter
                .get_match("youpi b youpi", &mut LineContext::default(), true)
                .unwrap(),
            None
        );
        assert_eq!(
            filter
                .get_match("insert  here", &mut LineContext::default(), true)
                .unwrap(),
            None
        );

        // two patterns in one regex
        filter = FilterMethodRegex::default();
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        assert!(filter.setup(&patterns).is_ok());
        assert_eq!(
            filter
                .get_match(
                    "insert b here and bouboubou there",
                    &mut LineContext::default(),
                    true
                )
                .unwrap(),
            Some(vec!("bouboubou".into(), "b".into()))
        );
        assert_eq!(
            filter
                .get_match(
                    "insert a here and bouboubou there",
                    &mut LineContext::default(),
                    true
                )
                .unwrap(),
            None
        );
        assert_eq!(
            filter
                .get_match(
                    "insert b here and boubou there",
                    &mut LineContext::default(),
                    true
                )
                .unwrap(),
            None
        );

        // multiple regexes with same pattern
        filter = FilterMethodRegex::default();
        filter.regex.push("insert <name> here".to_string());
        filter.regex.push("also add <name> there".to_string());
        assert!(filter.setup(&patterns).is_ok());
        assert_eq!(
            filter
                .get_match("insert a here", &mut LineContext::default(), true)
                .unwrap(),
            None
        );
        assert_eq!(
            filter
                .get_match("insert b here", &mut LineContext::default(), true)
                .unwrap(),
            Some(vec!("b".into()))
        );
        assert_eq!(
            filter
                .get_match("also add a there", &mut LineContext::default(), true)
                .unwrap(),
            None
        );
        assert_eq!(
            filter
                .get_match("also add b there", &mut LineContext::default(), true)
                .unwrap(),
            Some(vec!("b".into()))
        );

        // multiple regexes with same patterns
        filter = FilterMethodRegex::default();
        filter
            .regex
            .push("insert <name> here and <boubou> there".to_string());
        filter
            .regex
            .push("also add <boubou> here and <name> there".to_string());
        assert!(filter.setup(&patterns).is_ok());
        assert_eq!(
            filter
                .get_match(
                    "insert b here and bouboubou there",
                    &mut LineContext::default(),
                    true
                )
                .unwrap(),
            Some(vec!("bouboubou".into(), "b".into()))
        );
        assert_eq!(
            filter
                .get_match(
                    "also add bouboubou here and b there",
                    &mut LineContext::default(),
                    true
                )
                .unwrap(),
            Some(vec!("bouboubou".into(), "b".into()))
        );
        assert_eq!(
            filter
                .get_match(
                    "insert a here and bouboubou there",
                    &mut LineContext::default(),
                    true
                )
                .unwrap(),
            None
        );
        assert_eq!(
            filter
                .get_match(
                    "also add bouboubou here and a there",
                    &mut LineContext::default(),
                    true
                )
                .unwrap(),
            None
        );
        assert_eq!(
            filter
                .get_match(
                    "insert b here and boubou there",
                    &mut LineContext::default(),
                    true
                )
                .unwrap(),
            None
        );
        assert_eq!(
            filter
                .get_match(
                    "also add boubou here and b there",
                    &mut LineContext::default(),
                    true
                )
                .unwrap(),
            None
        );
    }
}
