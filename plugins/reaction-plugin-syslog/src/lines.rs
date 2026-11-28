//! Syslog line normalization.

use std::sync::Arc;

use crate::{config::Framing, log::LogThrottle};

/// Remove ANSI escape sequences (CSI and OSC) from a line.
fn strip_ansi(text: String) -> String {
    if !text.as_bytes().contains(&0x1b) {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                let mut prev_esc = false;
                for next in chars.by_ref() {
                    if next == '\u{7}' || (prev_esc && next == '\\') {
                        break;
                    }
                    prev_esc = next == '\x1b';
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Decode UTF-8 while dropping only invalid bytes.
fn decode_utf8(data: &[u8]) -> String {
    match std::str::from_utf8(data) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let mut out = String::with_capacity(data.len());
            let mut rest = data;
            while !rest.is_empty() {
                match std::str::from_utf8(rest) {
                    Ok(text) => {
                        out.push_str(text);
                        break;
                    }
                    Err(err) => {
                        let valid = err.valid_up_to();
                        if valid > 0 {
                            out.push_str(
                                std::str::from_utf8(&rest[..valid])
                                    .expect("valid_up_to returns a UTF-8 boundary"),
                            );
                        }
                        let skip = err.error_len().unwrap_or(rest.len() - valid);
                        rest = &rest[valid + skip..];
                    }
                }
            }
            out
        }
    }
}

/// Convert syslog bytes to a reaction line.
///
/// Trailing NUL and CR bytes are removed, invalid UTF-8 is dropped and ANSI
/// escape sequences are stripped so text filters remain usable.
pub fn to_text(data: &[u8]) -> String {
    let mut end = data.len();
    while end > 0 && (data[end - 1] == 0 || data[end - 1] == b'\r') {
        end -= 1;
    }
    strip_ansi(decode_utf8(&data[..end]))
}

/// Reassembles TCP syslog frames.
///
/// `Auto` supports both newline framing and octet-counted framing
/// (`<length> <message>`). `Newline` and `Octet` select one explicitly.
#[derive(Debug)]
pub struct TcpFramer {
    buf: Vec<u8>,
    max_bytes: usize,
    framing: Framing,
    warn: Arc<LogThrottle>,
}

impl TcpFramer {
    pub fn new(max_bytes: usize, framing: Framing, warn: Arc<LogThrottle>) -> Self {
        Self {
            buf: Vec::new(),
            max_bytes,
            framing,
            warn,
        }
    }

    pub fn push(&mut self, data: &[u8], out: &mut Vec<Vec<u8>>) {
        self.buf.extend_from_slice(data);
        loop {
            if self.buf.is_empty() {
                return;
            }
            if self.framing != Framing::Newline {
                if let Some((prefix_len, message_len)) = self.octet_length() {
                    if message_len > self.max_bytes {
                        log_warn_throttled!(
                            self.warn,
                            "tcp_oversize_frame",
                            "TCP frame of {message_len} bytes exceeds max_line_bytes, dropping"
                        );
                        match prefix_len
                            .checked_add(1)
                            .and_then(|total| total.checked_add(message_len))
                        {
                            Some(total) if self.buf.len() >= total => {
                                self.buf.drain(..total);
                            }
                            _ => {
                                self.buf.clear();
                                return;
                            }
                        }
                        continue;
                    }
                    let Some(total) = prefix_len
                        .checked_add(1)
                        .and_then(|total| total.checked_add(message_len))
                    else {
                        log_warn_throttled!(
                            self.warn,
                            "tcp_invalid_frame_length",
                            "TCP frame length is too large, dropping"
                        );
                        self.buf.clear();
                        return;
                    };
                    if self.buf.len() < total {
                        return;
                    }
                    out.push(self.buf[prefix_len + 1..total].to_vec());
                    self.buf.drain(..total);
                    continue;
                }
                if self.framing == Framing::Octet {
                    match self.buf.iter().position(|byte| *byte == b'\n') {
                        Some(index) => {
                            log_warn_throttled!(
                                self.warn,
                                "tcp_invalid_octet_prefix",
                                "TCP line has no octet length prefix, dropping"
                            );
                            self.buf.drain(..=index);
                        }
                        None if self.buf.len() > self.max_bytes => {
                            log_warn_throttled!(
                                self.warn,
                                "tcp_invalid_octet_prefix",
                                "TCP octet length prefix is too long, dropping"
                            );
                            self.buf.clear();
                        }
                        None => return,
                    }
                    continue;
                }
            }
            match self.buf.iter().position(|byte| *byte == b'\n') {
                Some(index) => {
                    if index > self.max_bytes {
                        log_warn_throttled!(
                            self.warn,
                            "tcp_oversize_line",
                            "TCP line exceeds max_line_bytes, dropping"
                        );
                    } else {
                        out.push(self.buf[..index].to_vec());
                    }
                    self.buf.drain(..=index);
                }
                None => {
                    if self.buf.len() > self.max_bytes {
                        log_warn_throttled!(
                            self.warn,
                            "tcp_oversize_line",
                            "TCP line exceeds max_line_bytes, dropping"
                        );
                        self.buf.clear();
                    }
                    return;
                }
            }
        }
    }

    pub fn flush(&mut self) -> Option<Vec<u8>> {
        (!self.buf.is_empty()).then(|| std::mem::take(&mut self.buf))
    }

    fn octet_length(&self) -> Option<(usize, usize)> {
        let space = self.buf.iter().position(|byte| !byte.is_ascii_digit())?;
        if self.buf[space] != b' ' || space == 0 {
            return None;
        }
        let length = std::str::from_utf8(&self.buf[..space]).ok()?.parse().ok()?;
        Some((space, length))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_framer(max_bytes: usize, framing: Framing) -> TcpFramer {
        TcpFramer::new(max_bytes, framing, Arc::new(LogThrottle::default()))
    }

    #[test]
    fn normalizes_syslog_bytes() {
        assert_eq!(to_text(b"hello\0"), "hello");
        assert_eq!(to_text(b"hello\r\0"), "hello");
        assert_eq!(to_text(b"a\xffb"), "ab");
        assert_eq!(to_text("\u{fffd}".as_bytes()), "\u{fffd}");
        assert_eq!(to_text("\x1b[31mhello\x1b[0m".as_bytes()), "hello");
    }

    #[test]
    fn newline_frames_across_chunks() {
        let mut framer = new_framer(1024, Framing::Auto);
        let mut out = Vec::new();
        framer.push(b"one\ntw", &mut out);
        framer.push(b"o\n", &mut out);
        assert_eq!(out, [b"one".to_vec(), b"two".to_vec()]);
        assert_eq!(framer.flush(), None);
    }

    #[test]
    fn newline_framing_keeps_digit_prefixed_line() {
        let mut framer = new_framer(1024, Framing::Newline);
        let mut out = Vec::new();
        framer.push(b"5 hello\n404 not found\n", &mut out);
        assert_eq!(out, [b"5 hello".to_vec(), b"404 not found".to_vec()]);
    }

    #[test]
    fn octet_counted_frames_across_chunks() {
        let mut framer = new_framer(1024, Framing::Auto);
        let mut out = Vec::new();
        framer.push(b"11 <165>1 ", &mut out);
        assert!(out.is_empty());
        framer.push(b"host5 hello", &mut out);
        assert_eq!(out, [b"<165>1 host".to_vec(), b"hello".to_vec()]);
    }

    #[test]
    fn octet_framing_drops_lines_without_length_prefix() {
        let mut framer = new_framer(1024, Framing::Octet);
        let mut out = Vec::new();
        framer.push(b"oops\n2 ok", &mut out);
        assert_eq!(out, [b"ok".to_vec()]);
    }

    #[test]
    fn oversized_frames_are_dropped() {
        let mut framer = new_framer(4, Framing::Auto);
        let mut out = Vec::new();
        framer.push(b"too-long\nok\n", &mut out);
        assert_eq!(out, [b"ok".to_vec()]);

        let mut framer = new_framer(4, Framing::Auto);
        let mut out = Vec::new();
        framer.push(b"10 12345678901234567890\nok\n", &mut out);
        assert_eq!(out, [b"ok".to_vec()]);
    }

    #[test]
    fn oversized_length_prefix_does_not_overflow() {
        let mut framer = new_framer(1024, Framing::Auto);
        let mut out = Vec::new();
        framer.push(format!("{} x", usize::MAX).as_bytes(), &mut out);
        assert!(out.is_empty());
        assert!(framer.buf.is_empty());

        framer.push(b"ok\n", &mut out);
        assert_eq!(out, [b"ok".to_vec()]);
    }
}
