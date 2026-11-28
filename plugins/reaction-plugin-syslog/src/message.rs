//! Syslog metadata shared by parsing, selection and templates.

use std::collections::BTreeMap;

const FACILITIES: &[&str] = &[
    "kern", "user", "mail", "daemon", "auth", "syslog", "lpr", "news", "uucp", "cron", "authpriv",
    "ftp", "ntp", "audit", "alert", "cron2", "local0", "local1", "local2", "local3", "local4",
    "local5", "local6", "local7",
];
const SEVERITIES: &[&str] = &[
    "emerg", "alert", "crit", "err", "warning", "notice", "info", "debug",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facility(u8);

impl Facility {
    pub fn from_code(code: u8) -> Option<Self> {
        ((code as usize) < FACILITIES.len()).then_some(Self(code))
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        let name = match normalized.as_str() {
            "kernel" => "kern",
            other => other,
        };
        FACILITIES
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| Self(index as u8))
    }

    pub fn as_str(self) -> &'static str {
        FACILITIES[self.0 as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Severity(u8);

impl Severity {
    pub fn from_code(code: u8) -> Option<Self> {
        ((code as usize) < SEVERITIES.len()).then_some(Self(code))
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        let name = match normalized.as_str() {
            "panic" => "emerg",
            "warn" => "warning",
            "error" => "err",
            other => other,
        };
        SEVERITIES
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| Self(index as u8))
    }

    pub fn as_str(self) -> &'static str {
        SEVERITIES[self.0 as usize]
    }
}

#[derive(Debug, Clone, Default)]
pub struct SyslogMeta {
    pub facility: Option<Facility>,
    pub severity: Option<Severity>,
    pub host: Option<String>,
    pub appname: Option<String>,
    pub procid: Option<String>,
    pub msgid: Option<String>,
    pub structured_data: BTreeMap<String, BTreeMap<String, String>>,
    pub source_addr: Option<String>,
}

use std::time::Duration;

use chrono::DateTime;

use crate::config::ParseMode;

#[derive(Debug, Clone)]
pub struct ParsedMessage {
    pub meta: SyslogMeta,
    pub text: String,
    pub timestamp: Option<Duration>,
}

pub fn parse(raw: &str, mode: ParseMode) -> ParsedMessage {
    match mode {
        ParseMode::Raw => fallback(raw),
        ParseMode::Rfc3164 => parse_rfc3164(raw).unwrap_or_else(|| fallback(raw)),
        ParseMode::Rfc5424 => parse_rfc5424(raw).unwrap_or_else(|| fallback(raw)),
        ParseMode::Auto => parse_rfc5424(raw)
            .or_else(|| parse_rfc3164(raw))
            .unwrap_or_else(|| fallback(raw)),
    }
}

fn fallback(raw: &str) -> ParsedMessage {
    ParsedMessage {
        meta: SyslogMeta::default(),
        text: raw.to_string(),
        timestamp: None,
    }
}

fn parse_pri(raw: &str) -> Option<(Facility, Severity, &str)> {
    let stripped = raw.strip_prefix('<')?;
    let end = stripped.find('>')?;
    let priority: u8 = stripped[..end].parse().ok()?;
    if priority > 191 {
        return None;
    }
    Some((
        Facility::from_code(priority / 8)?,
        Severity::from_code(priority % 8)?,
        &stripped[end + 1..],
    ))
}

fn optional(value: &str) -> Option<String> {
    (!value.is_empty() && value != "-").then(|| value.to_string())
}

fn parse_rfc5424(raw: &str) -> Option<ParsedMessage> {
    let (facility, severity, rest) = parse_pri(raw)?;
    let mut fields = rest.splitn(7, ' ');
    if fields.next()? != "1" {
        return None;
    }
    let timestamp = fields.next()?;
    let host = fields.next()?;
    let appname = fields.next()?;
    let procid = fields.next()?;
    let msgid = fields.next()?;
    let remaining = fields.next().unwrap_or("");
    let (sd, text) = split_structured_data(remaining)?;

    Some(ParsedMessage {
        meta: SyslogMeta {
            facility: Some(facility),
            severity: Some(severity),
            host: optional(host),
            appname: optional(appname),
            procid: optional(procid),
            msgid: optional(msgid),
            structured_data: parse_structured_data(sd).unwrap_or_default(),
            source_addr: None,
        },
        text: text.to_string(),
        timestamp: parse_timestamp(timestamp),
    })
}

fn split_structured_data(rest: &str) -> Option<(&str, &str)> {
    if rest.starts_with('-') {
        let end = rest.find(' ').unwrap_or(rest.len());
        return Some((&rest[..1], rest[end..].trim_start()));
    }
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    let mut in_quote = false;
    let mut escape = false;
    for (index, byte) in bytes.iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        match *byte {
            b'\\' if in_quote => escape = true,
            b'"' => in_quote = !in_quote,
            b'[' if !in_quote => depth += 1,
            b']' if !in_quote && depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let next = index + 1;
                    if next == bytes.len() {
                        return Some((rest[..next].trim_end(), &rest[next..]));
                    }
                    if bytes[next] == b' ' {
                        return Some((rest[..next].trim_end(), &rest[next + 1..]));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_structured_data(token: &str) -> Option<BTreeMap<String, BTreeMap<String, String>>> {
    let mut data = BTreeMap::new();
    let mut rest = token;
    while rest.starts_with('[') {
        let (element, remainder) = take_element(rest)?;
        let inner = &element[1..element.len() - 1];
        let split = inner.find(char::is_whitespace).unwrap_or(inner.len());
        let id = &inner[..split];
        if id.is_empty() {
            return None;
        }
        data.insert(id.to_string(), parse_params(&inner[split..]).ok()?);
        rest = remainder.trim_start();
    }
    (!data.is_empty()).then_some(data)
}

fn take_element(input: &str) -> Option<(&str, &str)> {
    let mut in_quote = false;
    let mut escape = false;
    for (index, c) in input.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match c {
            '\\' if in_quote => escape = true,
            '"' => in_quote = !in_quote,
            ']' if !in_quote => return Some((&input[..=index], &input[index + c.len_utf8()..])),
            _ => {}
        }
    }
    None
}

fn parse_params(input: &str) -> Result<BTreeMap<String, String>, ()> {
    let mut params = BTreeMap::new();
    let mut rest = input;
    loop {
        let trimmed = rest.trim_start();
        if trimmed.is_empty() {
            return Ok(params);
        }
        let eq = trimmed.find('=').ok_or(())?;
        let key = &trimmed[..eq];
        if key.is_empty() || key.chars().any(char::is_whitespace) {
            return Err(());
        }
        let after_quote = trimmed[eq + 1..].strip_prefix('"').ok_or(())?;
        let mut value = String::new();
        let mut escape = false;
        let mut end = None;
        for (index, c) in after_quote.char_indices() {
            if escape {
                value.push(c);
                escape = false;
                continue;
            }
            if c == '\\' {
                escape = true;
                continue;
            }
            if c == '"' {
                end = Some(index + c.len_utf8());
                break;
            }
            value.push(c);
        }
        let consumed = eq + 1 + 1 + end.ok_or(())?;
        params.insert(key.to_string(), value);
        rest = &trimmed[consumed..];
    }
}

fn parse_timestamp(value: &str) -> Option<Duration> {
    let parsed = DateTime::parse_from_rfc3339(value).ok()?;
    Some(Duration::from_secs(parsed.timestamp().max(0) as u64))
}

fn parse_rfc3164(raw: &str) -> Option<ParsedMessage> {
    let (facility, severity, rest) = parse_pri(raw)?;
    let head = rest.as_bytes().get(..15)?;
    if !valid_rfc3164_timestamp(head) {
        return None;
    }
    let after_timestamp = rest.get(15..)?.trim_start();
    let (host, after_host) = after_timestamp.split_once(' ')?;
    let header = after_host.trim_start();
    let colon = header.find(": ")?;
    let tag = &header[..colon];
    let text = &header[colon + 2..];
    let (appname, procid) = match tag.strip_suffix(']').and_then(|tag| tag.split_once('[')) {
        Some((appname, procid)) => (Some(appname.to_string()), Some(procid.to_string())),
        None => (Some(tag.to_string()), None),
    };

    Some(ParsedMessage {
        meta: SyslogMeta {
            facility: Some(facility),
            severity: Some(severity),
            host: optional(host),
            appname,
            procid,
            msgid: None,
            structured_data: BTreeMap::new(),
            source_addr: None,
        },
        text: text.to_string(),
        timestamp: None,
    })
}

fn valid_rfc3164_timestamp(bytes: &[u8]) -> bool {
    bytes.len() == 15
        && bytes[0..3].iter().all(u8::is_ascii_alphabetic)
        && bytes[3] == b' '
        && (bytes[4].is_ascii_digit() || bytes[4] == b' ')
        && bytes[5].is_ascii_digit()
        && bytes[6] == b' '
        && bytes[7..9].iter().all(u8::is_ascii_digit)
        && bytes[9] == b':'
        && bytes[10..12].iter().all(u8::is_ascii_digit)
        && bytes[12] == b':'
        && bytes[13..15].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facility_and_severity_names() {
        assert_eq!(Facility::parse("authpriv"), Facility::from_code(10));
        assert_eq!(
            Facility::parse("kernel"),
            Some(Facility::from_code(0).unwrap())
        );
        assert_eq!(Facility::parse("local7"), Facility::from_code(23));
        assert_eq!(Facility::parse("unknown"), None);

        assert_eq!(Severity::parse("warn"), Severity::from_code(4));
        assert_eq!(Severity::parse("error"), Severity::from_code(3));
        assert_eq!(Severity::parse("debug"), Severity::from_code(7));
        assert_eq!(Severity::parse("unknown"), None);
    }

    #[test]
    fn parse_rfc5424_message() {
        let raw = "<85>1 2003-10-11T22:14:15.003Z mymachine evntslog 1234 ID47 \
            [exampleSDID@32473 iut=\"3\" eventSource=\"Application\"] An event";
        let parsed = parse(raw, ParseMode::Auto);
        assert_eq!(parsed.meta.facility, Facility::parse("authpriv"));
        assert_eq!(parsed.meta.severity, Severity::parse("notice"));
        assert_eq!(parsed.meta.host.as_deref(), Some("mymachine"));
        assert_eq!(parsed.meta.appname.as_deref(), Some("evntslog"));
        assert_eq!(parsed.meta.procid.as_deref(), Some("1234"));
        assert_eq!(parsed.meta.msgid.as_deref(), Some("ID47"));
        assert_eq!(parsed.meta.structured_data["exampleSDID@32473"]["iut"], "3");
        assert_eq!(parsed.text, "An event");
        assert!(parsed.timestamp.is_some());
    }

    #[test]
    fn parse_rfc3164_message() {
        let raw = "<34>Feb  5 17:32:18 192.168.1.1 sshd[1234]: Failed password";
        let parsed = parse(raw, ParseMode::Auto);
        assert_eq!(parsed.meta.facility, Facility::parse("auth"));
        assert_eq!(parsed.meta.severity, Severity::parse("crit"));
        assert_eq!(parsed.meta.host.as_deref(), Some("192.168.1.1"));
        assert_eq!(parsed.meta.appname.as_deref(), Some("sshd"));
        assert_eq!(parsed.meta.procid.as_deref(), Some("1234"));
        assert_eq!(parsed.text, "Failed password");
        assert!(parsed.timestamp.is_none());
    }

    #[test]
    fn malformed_messages_fall_back_to_raw() {
        let parsed = parse("not syslog", ParseMode::Auto);
        assert_eq!(parsed.text, "not syslog");
        assert!(parsed.meta.facility.is_none());
        assert!(parsed.timestamp.is_none());
    }

    #[test]
    fn multibyte_rfc3164_candidate_falls_back_without_panic() {
        let raw = "<13>éééééééé";
        for mode in [ParseMode::Auto, ParseMode::Rfc3164] {
            let parsed = parse(raw, mode);
            assert_eq!(parsed.text, "<13>éééééééé");
            assert!(parsed.meta.facility.is_none());
        }
    }
}
