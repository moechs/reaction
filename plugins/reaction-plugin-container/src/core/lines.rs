//! Turning byte chunks into lines.

use std::time::SystemTime;

/// Reassembles lines from chunks of one output (stdout or stderr).
///
/// A line keeps the timestamp of the chunk it started in. Lines longer than
/// `max_bytes` are truncated, and the rest of the line is discarded until the
/// next newline.
#[derive(Debug)]
pub struct LineAssembler {
    buf: Vec<u8>,
    ts: Option<SystemTime>,
    in_line: bool,
    max_bytes: usize,
}

impl LineAssembler {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            buf: Vec::new(),
            ts: None,
            in_line: false,
            max_bytes,
        }
    }

    /// Feed one chunk. Completed lines (without `\n`) are pushed into `out`.
    pub fn push(
        &mut self,
        mut data: &[u8],
        ts: Option<SystemTime>,
        out: &mut Vec<(Vec<u8>, Option<SystemTime>)>,
    ) {
        while !data.is_empty() {
            if !self.in_line {
                self.in_line = true;
                self.ts = ts;
            }
            let (segment, rest, complete) = match data.iter().position(|b| *b == b'\n') {
                Some(i) => (&data[..i], &data[i + 1..], true),
                None => (data, &[][..], false),
            };
            let room = self.max_bytes.saturating_sub(self.buf.len());
            self.buf
                .extend_from_slice(&segment[..segment.len().min(room)]);
            if complete {
                out.push((std::mem::take(&mut self.buf), self.ts.take()));
                self.in_line = false;
            }
            data = rest;
        }
    }

    /// Return the pending partial line, if any (call at end of stream).
    pub fn flush(&mut self) -> Option<(Vec<u8>, Option<SystemTime>)> {
        if !self.in_line {
            return None;
        }
        self.in_line = false;
        Some((std::mem::take(&mut self.buf), self.ts.take()))
    }
}

/// Remove ANSI escape sequences (CSI and OSC) from a log line.
///
/// Applications that emit colored logs (e.g. `tracing_subscriber` with ANSI
/// enabled) insert escape sequences between field names, `=` and values. Those
/// sequences break literal-text filters such as `client_ip="<ip>"`.
///
/// Lines without an ESC byte are returned unchanged: the common case costs a
/// single vectorized scan ([`std::slice::contains`] uses memchr) and no
/// allocation beyond the line itself.
fn strip_ansi(text: String) -> String {
    if !text.as_bytes().contains(&0x1b) {
        return text;
    }
    let input = text.as_str();
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            // CSI: ESC [ ... final byte in @..~. Parameter bytes (0x30-0x3f)
            // and intermediate bytes (0x20-0x2f) are below @, so the first
            // character can be a terminator, e.g. in `\x1b[m`.
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... terminated by BEL or ST (ESC \)
            Some(']') => {
                chars.next();
                let mut prev_esc = false;
                for next in chars.by_ref() {
                    if next == '\u{7}' {
                        break;
                    }
                    if prev_esc && next == '\\' {
                        break;
                    }
                    prev_esc = next == '\x1b';
                }
            }
            // Other two-byte escape sequences.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Convert raw bytes to a reaction line, like reaction does for `cmd` streams:
/// invalid UTF-8 is dropped, and so is a trailing `\r` (TTY output). ANSI
/// escape sequences are also removed so colored logs remain filterable.
pub fn to_text(data: &[u8]) -> String {
    let data = data.strip_suffix(b"\r").unwrap_or(data);
    let text = String::from_utf8_lossy(data);
    let text = if text.contains(char::REPLACEMENT_CHARACTER) {
        text.replace(char::REPLACEMENT_CHARACTER, "")
    } else {
        text.into_owned()
    };
    strip_ansi(text)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn t(s: u64) -> Option<SystemTime> {
        Some(UNIX_EPOCH + Duration::from_secs(s))
    }

    #[test]
    fn split_and_join() {
        let mut a = LineAssembler::new(1024);
        let mut out = vec![];
        a.push(b"one\ntw", t(1), &mut out);
        a.push(b"o\nthree\n", t(2), &mut out);
        a.push(b"four", t(3), &mut out);
        assert_eq!(
            out,
            vec![
                (b"one".to_vec(), t(1)),
                (b"two".to_vec(), t(1)),
                (b"three".to_vec(), t(2)),
            ]
        );
        assert_eq!(a.flush(), Some((b"four".to_vec(), t(3))));
        assert_eq!(a.flush(), None);
    }

    #[test]
    fn empty_lines_are_kept() {
        let mut a = LineAssembler::new(1024);
        let mut out = vec![];
        a.push(b"\n\n", None, &mut out);
        assert_eq!(out, vec![(vec![], None), (vec![], None)]);
    }

    #[test]
    fn truncation() {
        let mut a = LineAssembler::new(4);
        let mut out = vec![];
        a.push(b"abcdefgh", t(1), &mut out);
        a.push(b"ijk\nxy\n", t(2), &mut out);
        assert_eq!(out, vec![(b"abcd".to_vec(), t(1)), (b"xy".to_vec(), t(2))]);
    }

    #[test]
    fn text() {
        assert_eq!(to_text(b"hello\r"), "hello");
        assert_eq!(to_text(b"a\xffb"), "ab");
        assert_eq!(to_text("été".as_bytes()), "été");
    }

    #[test]
    fn ansi_escape_codes_are_stripped() {
        // CSI sequences inserted between a field name, '=' and its value,
        // as produced by tracing_subscriber with ANSI colors enabled.
        let colored = "\x1b[2m2026-11-23T16:46:37Z\x1b[0m \x1b[33m WARN\x1b[0m \
            lorica::proxy_wiring: request completed \x1b[3mclient_ip\x1b[0m\x1b[2m=\x1b[0m\"127.0.0.1\" \
            \x1b[3mstatus\x1b[0m\x1b[2m=\x1b[0m403";
        assert_eq!(
            to_text(colored.as_bytes()),
            "2026-11-23T16:46:37Z  WARN lorica::proxy_wiring: request completed client_ip=\"127.0.0.1\" status=403"
        );

        // OSC sequences (window title, hyperlinks) are removed too.
        let osc = "\x1b]0;title\u{7}hello\x1b]8;;http://x\x1b\\world";
        assert_eq!(to_text(osc.as_bytes()), "helloworld");

        // CSI with no parameters (\x1b[m) must not eat the following text.
        assert_eq!(to_text(b"\x1b[mhello"), "hello");

        // A lone ESC at the end is dropped; normal text is untouched.
        assert_eq!(to_text(b"\x1b"), "");
        assert_eq!(to_text(b"plain text"), "plain text");
    }
}
