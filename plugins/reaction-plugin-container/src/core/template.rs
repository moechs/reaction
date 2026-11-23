//! Line prefix templates such as `"{name} "`.
//!
//! reaction lines are plain strings, so the only way to expose container
//! metadata to filters (and then to actions, through patterns) is to prepend it
//! to every line.
//!
//! Variables:
//! - `{id}`, `{short_id}`, `{name}`, `{image}`
//! - `{source}`: `stdout` or `stderr`
//! - `{label:KEY}`: value of label `KEY`, empty when missing
//! - backend-specific keys declared by the backend (see [`ContainerMeta::extra`])
//!
//! `{{` and `}}` produce literal braces.

use super::{meta::ContainerMeta, runtime::Source};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Id,
    ShortId,
    Name,
    Image,
    Source,
    Label(String),
    Extra(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Template {
    parts: Vec<Part>,
}

impl Template {
    /// Parse a template. `extra_keys` lists the backend-specific variables
    /// that are allowed in addition to the common ones.
    pub fn parse(template: &str, extra_keys: &[&str]) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut chars = template.chars().peekable();

        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    literal.push('{');
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    literal.push('}');
                }
                '{' => {
                    let mut var = String::new();
                    loop {
                        match chars.next() {
                            Some('}') => break,
                            Some(c) => var.push(c),
                            None => {
                                return Err(format!(
                                    "prefix {template:?}: unclosed '{{' (use '{{{{' for a literal brace)"
                                ));
                            }
                        }
                    }
                    if !literal.is_empty() {
                        parts.push(Part::Literal(std::mem::take(&mut literal)));
                    }
                    parts.push(Self::parse_var(template, &var, extra_keys)?);
                }
                '}' => {
                    return Err(format!(
                        "prefix {template:?}: unmatched '}}' (use '}}}}' for a literal brace)"
                    ));
                }
                c => literal.push(c),
            }
        }
        if !literal.is_empty() {
            parts.push(Part::Literal(literal));
        }
        Ok(Self { parts })
    }

    fn parse_var(template: &str, var: &str, extra_keys: &[&str]) -> Result<Part, String> {
        Ok(match var {
            "id" => Part::Id,
            "short_id" => Part::ShortId,
            "name" => Part::Name,
            "image" => Part::Image,
            "source" => Part::Source,
            _ => {
                if let Some(key) = var.strip_prefix("label:") {
                    if key.is_empty() {
                        return Err(format!("prefix {template:?}: empty label key"));
                    }
                    Part::Label(key.to_string())
                } else if extra_keys.contains(&var) {
                    Part::Extra(var.to_string())
                } else {
                    return Err(format!(
                        "prefix {template:?}: unknown variable {{{var}}}. known variables: \
                         id, short_id, name, image, source, label:KEY{}",
                        extra_keys
                            .iter()
                            .map(|k| format!(", {k}"))
                            .collect::<String>()
                    ));
                }
            }
        })
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    pub fn render(&self, meta: &ContainerMeta, source: Source, out: &mut String) {
        let start = out.len();
        for part in &self.parts {
            match part {
                Part::Literal(s) => out.push_str(s),
                Part::Id => push_safe(out, &meta.id),
                Part::ShortId => push_safe(out, meta.short_id()),
                Part::Name => push_safe(out, &meta.name),
                Part::Image => push_safe(out, &meta.image),
                Part::Source => out.push_str(source.as_str()),
                Part::Label(key) => {
                    if let Some(v) = meta.labels.get(key) {
                        push_safe(out, v)
                    }
                }
                Part::Extra(key) => {
                    if let Some(v) = meta.extra.get(key) {
                        push_safe(out, v)
                    }
                }
            }
        }
        // Container labels are untrusted input: keep a single metadata value
        // from inflating every log line and bounding filter/action memory.
        if out.len() - start > MAX_RENDERED_PREFIX {
            let mut end = start + MAX_RENDERED_PREFIX;
            while !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
        }
    }
}

const MAX_RENDERED_PREFIX: usize = 4096;

/// Append metadata without control characters or ANSI escape sequences, which
/// could otherwise inject fake log content into filters.
fn push_safe(out: &mut String, value: &str) {
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                // CSI sequences start with '['; the first byte after ESC is
                // not a terminator even though it is in the printable range.
                let mut first = true;
                for next in chars.by_ref() {
                    if !first && ('@'..='~').contains(&next) {
                        break;
                    }
                    first = false;
                }
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(t: &str, source: Source) -> String {
        let meta = ContainerMeta {
            id: "0123456789abcdef0123".into(),
            name: "web".into(),
            image: "nginx:1".into(),
            labels: [("app".to_string(), "shop".to_string())].into(),
            extra: [("pod".to_string(), "web-0".to_string())].into(),
        };
        let mut out = String::new();
        Template::parse(t, &["pod"])
            .unwrap()
            .render(&meta, source, &mut out);
        out
    }

    #[test]
    fn variables() {
        assert_eq!(
            render("{name}/{short_id}/{image}/{source}: ", Source::Stderr),
            "web/0123456789ab/nginx:1/stderr: "
        );
        assert_eq!(render("{id}", Source::Stdout), "0123456789abcdef0123");
        assert_eq!(
            render("[{label:app}|{label:nope}]", Source::Stdout),
            "[shop|]"
        );
        assert_eq!(render("{pod}", Source::Stdout), "web-0");
        assert_eq!(render("{{{name}}} ", Source::Stdout), "{web} ");
        assert_eq!(render("", Source::Stdout), "");
    }

    #[test]
    fn metadata_cannot_inject_log_lines() {
        let mut meta = ContainerMeta {
            id: "0123456789abcdef0123".into(),
            name: "web".into(),
            image: "nginx:1".into(),
            labels: Default::default(),
            extra: Default::default(),
        };
        meta.labels
            .insert("tenant".into(), "web\nfake 203.0.113.1\x1b[31m\r\0".into());
        let mut out = String::new();
        Template::parse("{label:tenant}", &[])
            .unwrap()
            .render(&meta, Source::Stdout, &mut out);
        assert_eq!(out, "webfake 203.0.113.1");
    }

    #[test]
    fn errors() {
        assert!(Template::parse("{nope}", &[]).is_err());
        assert!(Template::parse("{pod}", &[]).is_err());
        assert!(Template::parse("{name", &[]).is_err());
        assert!(Template::parse("name}", &[]).is_err());
        assert!(Template::parse("{label:}", &[]).is_err());
        assert!(Template::parse("{}", &[]).is_err());
    }
}
