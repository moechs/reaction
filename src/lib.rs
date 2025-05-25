#![warn(
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unwrap_used,
    unsafe_code
)]
#![allow(clippy::upper_case_acronyms, clippy::mutable_key_type)]

// Allow unwrap in tests
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod cli;
pub mod client;
pub mod concepts;
pub mod daemon;
pub mod protocol;
pub mod tests;
pub mod treedb;
