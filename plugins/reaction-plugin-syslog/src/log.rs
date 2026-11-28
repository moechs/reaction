#![allow(unused_macros)]

//! Runtime logging on stderr.
//!
//! reaction captures the plugin's stderr line by line and parses an optional
//! level prefix (`DEBUG `, `INFO `, `WARN `, `ERROR `). Stdin and stdout are
//! reserved for the remoc transport and must never be written to.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

/// Allows repeated diagnostics caused by untrusted input to be logged at most
/// once per key and interval.
#[derive(Debug)]
pub struct LogThrottle {
    last: Mutex<HashMap<&'static str, Instant>>,
}

impl Default for LogThrottle {
    fn default() -> Self {
        Self {
            last: Mutex::new(HashMap::new()),
        }
    }
}

impl LogThrottle {
    const INTERVAL: Duration = Duration::from_secs(1);

    pub fn allow(&self, key: &'static str) -> bool {
        let mut last = self.last.lock().expect("log throttle poisoned");
        let now = Instant::now();
        match last.get(key) {
            Some(previous) if now.duration_since(*previous) < Self::INTERVAL => false,
            _ => {
                last.insert(key, now);
                true
            }
        }
    }
}

macro_rules! log_debug {
    ($($arg:tt)*) => { eprintln!("DEBUG {}", format_args!($($arg)*)) };
}
macro_rules! log_info {
    ($($arg:tt)*) => { eprintln!("INFO {}", format_args!($($arg)*)) };
}
macro_rules! log_warn {
    ($($arg:tt)*) => { eprintln!("WARN {}", format_args!($($arg)*)) };
}
macro_rules! log_error {
    ($($arg:tt)*) => { eprintln!("ERROR {}", format_args!($($arg)*)) };
}
macro_rules! log_warn_throttled {
    ($throttle:expr, $key:expr, $($arg:tt)*) => {
        if $throttle.allow($key) {
            eprintln!("WARN {}", format_args!($($arg)*));
        }
    };
}
macro_rules! log_error_throttled {
    ($throttle:expr, $key:expr, $($arg:tt)*) => {
        if $throttle.allow($key) {
            eprintln!("ERROR {}", format_args!($($arg)*));
        }
    };
}
