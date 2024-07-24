#![warn(
    missing_docs,
    clippy::unwrap_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::undocumented_unsafe_blocks
)]
#![forbid(unsafe_code)]

//! Hey it's the doc!
use clap::Parser;
use regex::Regex;

mod action;
mod filter;
mod pattern;
mod stream;

mod cli;
mod client;
mod config;
mod daemon;
mod logger;
mod parse_duration;

use cli::{Cli, Command};
use client::{flush, show, test_regex};
use daemon::daemon;

fn main() {
    cli::NamedRegex {
        regex: Regex::new(".").unwrap(),
        name: "test".to_string(),
    };

    let cli = Cli::parse();

    match cli.command {
        Command::Start {
            config,
            loglevel,
            socket,
        } => {
            daemon(&config, loglevel, &socket);
        }
        Command::Show {
            socket,
            format,
            limit,
            pattern,
            patterns,
        } => show(&socket, format, &limit, &pattern, &patterns),
        Command::Flush {
            socket,
            format,
            limit,
            pattern,
            patterns,
        } => flush(&socket, format, &limit, &pattern, &patterns),
        Command::TestRegex {
            config,
            regex,
            line,
        } => test_regex(&config, &regex, &line),
    }
}
