//! Read position of one (stream, container) pair, used to resume after a
//! reconnection or a container restart without duplicating lines.
//!
//! Runtimes such as Docker only accept a coarse `since` (whole seconds), so a
//! reopened log stream replays some lines that were already sent. While
//! replaying, lines older than the cursor are dropped, and so are lines whose
//! content was already seen at the cursor timestamp. Once a newer line shows
//! up the replay is over and everything passes, even if timestamps go
//! backwards later (clock adjustments must not drop live lines).

use std::{collections::VecDeque, time::SystemTime};

/// Number of recent line hashes kept at the cursor timestamp. Docker's
/// timestamps have nanosecond precision but reconnects can still produce more
/// lines than the window; in that case the oldest duplicate may be delivered
/// again, which is preferable to dropping arbitrary new lines.
const DEDUP_WINDOW: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// Timestamp of the most recent line sent.
    pub ts: SystemTime,
    /// Hashes of the most recent lines sent with exactly `ts`.
    hashes: VecDeque<u64>,
}

impl Cursor {
    pub fn new(ts: SystemTime) -> Self {
        Self {
            ts,
            hashes: VecDeque::new(),
        }
    }
}

/// Deduplication state of one open log stream.
#[derive(Debug)]
pub struct Replay {
    cursor: Cursor,
    replaying: bool,
}

impl Replay {
    pub fn new(cursor: Cursor) -> Self {
        Self {
            cursor,
            replaying: true,
        }
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor.clone()
    }

    /// Returns whether a line with this timestamp and hash must be sent, and
    /// updates the cursor. `hash` identifies the line content, so lines that
    /// share the same coarse timestamp can still be deduplicated.
    pub fn accept(&mut self, ts: SystemTime, hash: u64) -> bool {
        if self.replaying {
            if ts < self.cursor.ts {
                return false;
            }
            if ts == self.cursor.ts {
                if self.cursor.hashes.contains(&hash) {
                    return false;
                }
                self.remember(hash);
                return true;
            }
            self.replaying = false;
        }

        if ts >= self.cursor.ts {
            if ts > self.cursor.ts {
                self.cursor = Cursor::new(ts);
            }
            self.remember(hash);
        }
        true
    }

    fn remember(&mut self, hash: u64) {
        if self.cursor.hashes.contains(&hash) {
            return;
        }
        if self.cursor.hashes.len() == DEDUP_WINDOW {
            self.cursor.hashes.pop_front();
        }
        self.cursor.hashes.push_back(hash);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn t(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    #[test]
    fn baseline_drops_old_lines() {
        let mut r = Replay::new(Cursor::new(t(1000)));
        assert!(!r.accept(t(500), 1));
        assert!(r.accept(t(1000), 2));
        assert!(r.accept(t(1500), 3));
        assert_eq!(r.cursor().ts, t(1500));
    }

    #[test]
    fn resume_without_duplicates() {
        // First connection sends 3 lines, two of them with the same timestamp.
        let mut r = Replay::new(Cursor::new(t(0)));
        for (ts, hash) in [(t(1000), 1), (t(2000), 2), (t(2000), 3)] {
            assert!(r.accept(ts, hash));
        }
        let cursor = r.cursor();

        // Reconnection replays those lines plus a new one at the same
        // timestamp and a newer line: only the new ones must be accepted.
        let mut r = Replay::new(cursor);
        let replayed = [
            (t(1000), 1),
            (t(2000), 2),
            (t(2000), 3),
            (t(2000), 4),
            (t(3000), 5),
        ];
        let accepted: Vec<_> = replayed
            .into_iter()
            .map(|(ts, hash)| r.accept(ts, hash))
            .collect();
        assert_eq!(accepted, [false, false, false, true, true]);
    }

    #[test]
    fn live_lines_pass_even_if_clock_goes_back() {
        let mut r = Replay::new(Cursor::new(t(1000)));
        assert!(r.accept(t(2000), 1));
        assert!(r.accept(t(1500), 2));
        assert_eq!(r.cursor().ts, t(2000));
    }
}
