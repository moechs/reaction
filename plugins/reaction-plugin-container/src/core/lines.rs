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

/// Convert raw bytes to a reaction line, like reaction does for `cmd` streams:
/// invalid UTF-8 is dropped, and so is a trailing `\r` (TTY output).
pub fn to_text(data: &[u8]) -> String {
    let data = data.strip_suffix(b"\r").unwrap_or(data);
    let text = String::from_utf8_lossy(data);
    if text.contains(char::REPLACEMENT_CHARACTER) {
        text.replace(char::REPLACEMENT_CHARACTER, "")
    } else {
        text.into_owned()
    }
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
}
