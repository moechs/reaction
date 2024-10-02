#![warn(
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unwrap_used
)]
#![allow(clippy::upper_case_acronyms, clippy::mutable_key_type)]
#![forbid(unsafe_code)]

pub mod client;
pub mod concepts;
pub mod daemon;
pub mod tests;
pub mod utils;
