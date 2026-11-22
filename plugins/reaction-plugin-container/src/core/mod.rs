//! Runtime-agnostic logic shared by every backend.
//!
//! Nothing in this module knows about Docker: backends implement
//! [`runtime::Runtime`] and the [`supervisor`] does the rest.

pub mod config;
pub mod cursor;
pub mod lines;
pub mod meta;
pub mod runtime;
pub mod selector;
pub mod supervisor;
pub mod template;
