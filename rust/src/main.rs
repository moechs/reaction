use clap::Parser;

use reaction::client::{request, test_regex};
use reaction::concepts::Order;
use reaction::daemon::daemon;
use reaction::utils::cli::{Cli, Command};

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
            daemon(config, loglevel, socket);
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
