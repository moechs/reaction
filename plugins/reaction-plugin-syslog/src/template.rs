//! Safe prefix templates for syslog metadata.

use crate::message::SyslogMeta;

const MAX_RENDERED_PREFIX: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Field(String),
}

#[derive(Debug, Clone, Default)]
pub struct Template {
    parts: Vec<Part>,
}

impl Template {
    pub fn parse(input: &str) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut chars = input.chars().peekable();

        while let Some(c) = chars.next() {
            match c {
                '{' => {
                    if chars.peek() == Some(&'{') {
                        chars.next();
                        text.push('{');
                        continue;
                    }
                    let mut field = String::new();
                    let mut closed = false;
                    for next in chars.by_ref() {
                        if next == '}' {
                            closed = true;
                            break;
                        }
                        field.push(next);
                    }
                    if !closed {
                        return Err("prefix template has an unclosed '{'".into());
                    }
                    validate_field(&field)?;
                    if !text.is_empty() {
                        parts.push(Part::Text(std::mem::take(&mut text)));
                    }
                    parts.push(Part::Field(field));
                }
                '}' => {
                    if chars.peek() == Some(&'}') {
                        chars.next();
                        text.push('}');
                    } else {
                        return Err("prefix template has a lone '}'".into());
                    }
                }
                other => text.push(other),
            }
        }
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        Ok(Self { parts })
    }

    pub fn render(&self, meta: &SyslogMeta) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Text(text) => push_safe(&mut out, text),
                Part::Field(field) => push_safe(&mut out, &field_value(meta, field)),
            }
            if out.len() >= MAX_RENDERED_PREFIX {
                let mut end = MAX_RENDERED_PREFIX.min(out.len());
                while !out.is_char_boundary(end) {
                    end -= 1;
                }
                out.truncate(end);
                break;
            }
        }
        out
    }
}

fn validate_field(field: &str) -> Result<(), String> {
    let known = matches!(
        field,
        "facility" | "severity" | "host" | "appname" | "procid" | "msgid" | "source_addr"
    );
    if known {
        return Ok(());
    }
    if let Some(key) = field.strip_prefix("sd:") {
        if key.is_empty() {
            return Err("prefix template has an empty structured-data key".into());
        }
        return Ok(());
    }
    Err(format!("prefix template has unknown variable {{{field}}}"))
}

fn field_value(meta: &SyslogMeta, field: &str) -> String {
    match field {
        "facility" => meta
            .facility
            .map(|v| v.as_str().to_string())
            .unwrap_or_default(),
        "severity" => meta
            .severity
            .map(|v| v.as_str().to_string())
            .unwrap_or_default(),
        "host" => meta.host.clone().unwrap_or_default(),
        "appname" => meta.appname.clone().unwrap_or_default(),
        "procid" => meta.procid.clone().unwrap_or_default(),
        "msgid" => meta.msgid.clone().unwrap_or_default(),
        "source_addr" => meta.source_addr.clone().unwrap_or_default(),
        other => other
            .strip_prefix("sd:")
            .and_then(|key| {
                meta.structured_data
                    .values()
                    .find_map(|params| params.get(key))
            })
            .cloned()
            .unwrap_or_default(),
    }
}

fn push_safe(out: &mut String, value: &str) {
    for c in value.chars() {
        if !c.is_control() {
            out.push(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Facility, Severity};

    #[test]
    fn parse_and_render() {
        let template = Template::parse("{facility}.{severity} {appname}[{procid}]: ").unwrap();
        let meta = SyslogMeta {
            facility: Facility::parse("auth"),
            severity: Severity::parse("err"),
            appname: Some("sshd".into()),
            procid: Some("123".into()),
            ..Default::default()
        };
        assert_eq!(template.render(&meta), "auth.err sshd[123]: ");
    }

    #[test]
    fn literals_and_structured_data() {
        let template = Template::parse("{{host}} {sd:origin}\n").unwrap();
        let mut meta = SyslogMeta::default();
        meta.structured_data
            .insert("meta".into(), [("origin".into(), "edge".into())].into());
        assert_eq!(template.render(&meta), "{host} edge");
    }

    #[test]
    fn invalid_templates() {
        assert!(Template::parse("{unknown}").is_err());
        assert!(Template::parse("{sd:}").is_err());
        assert!(Template::parse("{host").is_err());
        assert!(Template::parse("host}").is_err());
    }
}
