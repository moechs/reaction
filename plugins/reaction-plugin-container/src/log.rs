//! Runtime logging.
//!
//! reaction captures the plugin's stderr line by line and parses an optional
//! level prefix (`DEBUG `, `INFO `, `WARN `, `ERROR `). Stdin and stdout are
//! reserved for the remoc transport and must never be written to.

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
