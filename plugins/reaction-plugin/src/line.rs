//! Helper module that permits to use templated lines (ie. `bad password for <ip>`), like in Stream's and Action's `cmd`.
//!
//! Corresponding reaction core settings:
//! - [Stream's `cmd`](https://reaction.ppom.me/reference.html#cmd)
//! - [Action's `cmd`](https://reaction.ppom.me/reference.html#cmd-1)
//!
#[derive(Debug, PartialEq, Eq)]
enum SendItem {
    Index(usize),
    Str(String),
}

impl SendItem {
    fn min_size(&self) -> usize {
        match self {
            Self::Index(_) => 0,
            Self::Str(s) => s.len(),
        }
    }
}

/// Helper struct that permits to transform a template line with patterns into an instantiated line from a match.
///
/// Useful when you permit the user to reconstruct lines from an action, like in reaction's native actions and in the virtual plugin:
/// ```yaml
/// actions:
///   native:
///     cmd: ["iptables", "...", "<ip>"]
///
///   virtual:
///     type: virtual
///     options:
///       send: "<ip>: bad password on user <user>"
///       to: "my_virtual_stream"
/// ```
///
/// Usage example:
/// ```
/// # use reaction_plugin::line::PatternLine;
/// #
/// let template = "<ip>: bad password on user <user>".to_string();
/// let patterns = vec!["ip".to_string(), "user".to_string()];
/// let pattern_line = PatternLine::new(template, patterns);
///
/// assert_eq!(
///   pattern_line.line(vec!["1.2.3.4".to_string(), "root".to_string()]),
///   "1.2.3.4: bad password on user root".to_string(),
/// );
/// ```
///
/// You can find full examples in those plugins:
/// `reaction-plugin-virtual`,
/// `reaction-plugin-cluster`.
///
#[derive(Debug)]
pub struct PatternLine {
    line: Vec<SendItem>,
    min_size: usize,
}

impl PatternLine {
    /// Construct [`PatternLine`] from a template line and the list of patterns of the underlying [Filter](https://reaction.ppom.me/reference.html#filter).
    ///
    /// This list of patterns comes from [`super::ActionConfig`].
    pub fn new(template: String, patterns: Vec<String>) -> Self {
        let line = Self::_from(patterns, Vec::from([SendItem::Str(template)]));
        Self {
            min_size: line.iter().map(SendItem::min_size).sum(),
            line,
        }
    }
    fn _from(mut patterns: Vec<String>, acc: Vec<SendItem>) -> Vec<SendItem> {
        match patterns.pop() {
            None => acc,
            Some(pattern) => {
                let enclosed_pattern = format!("<{pattern}>");
                let acc = acc
                    .into_iter()
                    .flat_map(|item| match &item {
                        SendItem::Index(_) => vec![item],
                        SendItem::Str(str) => match str.find(&enclosed_pattern) {
                            Some(i) => {
                                let pattern_index = patterns.len();
                                let mut ret = vec![];

                                let (left, mid) = str.split_at(i);
                                if !left.is_empty() {
                                    ret.push(SendItem::Str(left.into()))
                                }

                                ret.push(SendItem::Index(pattern_index));

                                if mid.len() > enclosed_pattern.len() {
                                    let (_, right) = mid.split_at(enclosed_pattern.len());
                                    ret.push(SendItem::Str(right.into()))
                                }

                                ret
                            }
                            None => vec![item],
                        },
                    })
                    .collect();
                Self::_from(patterns, acc)
            }
        }
    }

    pub fn line(&self, match_: Vec<String>) -> String {
        let mut res = String::with_capacity(self.min_size);
        for item in &self.line {
            match item {
                SendItem::Index(i) => {
                    if let Some(element) = match_.get(*i) {
                        res.push_str(element);
                    }
                }
                SendItem::Str(str) => res.push_str(str),
            }
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use crate::line::{PatternLine, SendItem};

    #[test]
    fn line_0_pattern() {
        let msg = "my message".to_string();
        let line = PatternLine::new(msg.clone(), vec![]);
        assert_eq!(line.line, vec![SendItem::Str(msg.clone())]);
        assert_eq!(line.min_size, msg.len());
        assert_eq!(line.line(vec![]), msg.clone());
    }

    #[test]
    fn line_1_pattern() {
        let patterns = vec![
            "ignored".into(),
            "oh".into(),
            "ignored".into(),
            "my".into(),
            "test".into(),
        ];

        let matches = vec!["yay", "oh", "my", "test", "<oh>", "<my>", "<test>"];

        let tests = [
            (
                "<oh> my test",
                1,
                vec![SendItem::Index(1), SendItem::Str(" my test".into())],
                vec![
                    ("yay", "yay my test"),
                    ("oh", "oh my test"),
                    ("my", "my my test"),
                    ("test", "test my test"),
                    ("<oh>", "<oh> my test"),
                    ("<my>", "<my> my test"),
                    ("<test>", "<test> my test"),
                ],
            ),
            (
                "oh <my> test",
                3,
                vec![
                    SendItem::Str("oh ".into()),
                    SendItem::Index(3),
                    SendItem::Str(" test".into()),
                ],
                vec![
                    ("yay", "oh yay test"),
                    ("oh", "oh oh test"),
                    ("my", "oh my test"),
                    ("test", "oh test test"),
                    ("<oh>", "oh <oh> test"),
                    ("<my>", "oh <my> test"),
                    ("<test>", "oh <test> test"),
                ],
            ),
            (
                "oh my <test>",
                4,
                vec![SendItem::Str("oh my ".into()), SendItem::Index(4)],
                vec![
                    ("yay", "oh my yay"),
                    ("oh", "oh my oh"),
                    ("my", "oh my my"),
                    ("test", "oh my test"),
                    ("<oh>", "oh my <oh>"),
                    ("<my>", "oh my <my>"),
                    ("<test>", "oh my <test>"),
                ],
            ),
        ];

        for (msg, index, expected_pl, lines) in tests {
            let pattern_line = PatternLine::new(msg.to_string(), patterns.clone());
            assert_eq!(pattern_line.line, expected_pl);

            for (match_element, line) in lines {
                for match_default in &matches {
                    let mut match_ = vec![
                        match_default.to_string(),
                        match_default.to_string(),
                        match_default.to_string(),
                        match_default.to_string(),
                        match_default.to_string(),
                    ];
                    match_[index] = match_element.to_string();
                    assert_eq!(
                        pattern_line.line(match_.clone()),
                        line,
                        "match: {match_:?}, pattern_line: {pattern_line:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn line_2_pattern() {
        let pattern_line = PatternLine::new("<a> ; <b>".into(), vec!["a".into(), "b".into()]);

        let matches = ["a", "b", "ab", "<a>", "<b>"];
        for a in &matches {
            for b in &matches {
                assert_eq!(
                    pattern_line.line(vec![a.to_string(), b.to_string()]),
                    format!("{a} ; {b}"),
                );
            }
        }
    }
}
