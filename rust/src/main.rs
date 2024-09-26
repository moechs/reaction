#![warn(
    missing_docs,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unwrap_used
)]
#![allow(clippy::upper_case_acronyms, clippy::mutable_key_type)]
#![forbid(unsafe_code)]

//! TODO document a bit
use clap::Parser;

mod client;
mod concepts;
mod daemon;

mod tests;
mod utils;

use client::{flush, show, test_regex};
use daemon::daemon;
use utils::cli::{Cli, Command};

fn main() {
    // Show a nice message when reaction panics
    std::panic::set_hook(Box::new(move |panic_info| {
        eprintln!("ERROR internal reaction error: panic");
        eprintln!("{}", panic_info);
        eprintln!();
        eprintln!("This is likely a bug in reaction. Please report it on the issue tracker:");
        eprintln!("https://framagit.org/ppom/reaction/-/issues");
        eprintln!();
        eprintln!("Please include the last log messages in your bug report, as well as any");
        eprintln!("relevant information on the context in which reaction was running when");
        eprintln!("this error occurred.");
    }));

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
