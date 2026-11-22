//! Read position of one (stream, container) pair, used to resume after a
//! reconnection or a container restart without duplicating lines.
//!
//! Runtimes such as Docker only accept a coarse `since` (whole seconds), so a
//! reopened log stream replays some lines that were already sent. While
//! replaying, lines older than the cursor are dropped, and so are the first
//! `seen_at_ts` lines carrying exactly the cursor timestamp. Once a newer line
//! shows up the replay is over and everything passes, even if timestamps go
//! backwards later (clock adjustments must not drop live lines).

use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// Timestamp of the most recent line sent.
    pub ts: SystemTime,
    /// How many lines with exactly `ts` were sent.
    pub seen_at_ts: usize,
}

impl Cursor {
    pub fn new(ts: SystemTime) -> Self {
        Self { ts, seen_at_ts: 0 }
    }
}

/// Deduplication state of one open log stream.
#[derive(Debug)]
pub struct Replay {
    cursor: Cursor,
    replaying: bool,
    skip_at_ts: usize,
}

impl Replay {
    pub fn new(cursor: Cursor) -> Self {
        Self {
            cursor,
            replaying: true,
            skip_at_ts: cursor.seen_at_ts,
        }
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Returns whether a line with this timestamp must be sent, and updates the cursor.
    /// Lines without timestamp always pass and leave the cursor untouched.
    pub fn accept(&mut self, ts: Option<SystemTime>) -> bool {
        let Some(ts) = ts else { return true };

        if self.replaying {
            if ts < self.cursor.ts {
                return false;
            }
            if ts == self.cursor.ts {
                if self.skip_at_ts > 0 {
                    self.skip_at_ts -= 1;
                    return false;
                }
                self.cursor.seen_at_ts += 1;
                return true;
            }
            self.replaying = false;
        }

        if ts > self.cursor.ts {
            self.cursor = Cursor { ts, seen_at_ts: 1 };
        } else if ts == self.cursor.ts {
            self.cursor.seen_at_ts += 1;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn t(ms: u64) -> Option<SystemTime> {
        Some(UNIX_EPOCH + Duration::from_millis(ms))
    }

    #[test]
    fn baseline_drops_old_lines() {
        let mut r = Replay::new(Cursor::new(t(1000).unwrap()));
        assert!(!r.accept(t(500)));
        assert!(r.accept(t(1000)));
        assert!(r.accept(t(1500)));
        assert!(r.accept(None));
        assert_eq!(
            r.cursor(),
            Cursor {
                ts: t(1500).unwrap(),
                seen_at_ts: 1
            }
        );
    }

    #[test]
    fn resume_without_duplicates() {
        // First connection sends 3 lines, two of them with the same timestamp.
        let mut r = Replay::new(Cursor::new(t(0).unwrap()));
        for ts in [t(1000), t(2000), t(2000)] {
            assert!(r.accept(ts));
        }
        let cursor = r.cursor();
        assert_eq!(cursor.seen_at_ts, 2);

        // Reconnection replays from the start of the second, plus a new line
        // that has the same timestamp as the last one sent.
        let mut r = Replay::new(cursor);
        let replayed = [t(1000), t(2000), t(2000), t(2000), t(3000)];
        let accepted: Vec<_> = replayed.into_iter().map(|ts| r.accept(ts)).collect();
        assert_eq!(accepted, [false, false, false, true, true]);
    }

    #[test]
    fn live_lines_pass_even_if_clock_goes_back() {
        let mut r = Replay::new(Cursor::new(t(1000).unwrap()));
        assert!(r.accept(t(2000)));
        assert!(r.accept(t(1500)));
        assert_eq!(r.cursor().ts, t(2000).unwrap());
    }
}
