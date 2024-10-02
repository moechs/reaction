use std::process::exit;

use clap::Parser;

use log::Level;
use reaction::client::{request, test_regex};
use reaction::concepts::Order;
use reaction::daemon::daemon;
use reaction::utils::cli::{Cli, Command};
use reaction::utils::SimpleLogger;

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

    {
        // Set log level
        let level = if let Command::Start {
            loglevel,
            config: _,
            socket: _,
        } = cli.command
        {
            loglevel
        } else {
            Level::Debug
        };
        if let Err(err) = SimpleLogger::init(level) {
            eprintln!("ERROR could not initialize logging: {err}");
            exit(1);
        }
    }

    match cli.command {
        Command::Start {
            config,
            loglevel: _,
            socket,
        } => {
            exit(match daemon(config, socket) {
                true => 0,
                false => 1,
            });
        }
        Command::Show {
            socket,
            format,
            limit,
            patterns,
        } => request(socket, format, limit, patterns, Order::Show),
        Command::Flush {
            socket,
            format,
            limit,
            patterns,
        } => request(socket, format, limit, patterns, Order::Flush),
        Command::TestRegex {
            config,
            regex,
            line,
        } => test_regex(config, regex, line),
    }
}
