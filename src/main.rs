use std::process::exit;

use clap::Parser;
use reaction::{
    cli::{Cli, Format, SubCommand},
    client::{request, test_config, test_filter, test_regex},
    daemon::daemon,
    protocol::Order,
};

#[tokio::main]
async fn main() {
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

    // console_subscriber::init();

    let cli = Cli::parse();

    let result = match cli.command {
        SubCommand::Start {
            loglevel,
            config,
            socket,
        } => {
            exit(daemon(config, loglevel, socket).await);
        }
        SubCommand::Show {
            socket,
            format,
            limit,
            patterns,
        } => request(socket, format, limit, patterns, Order::Show).await,
        SubCommand::Flush {
            socket,
            format,
            limit,
            patterns,
        } => request(socket, format, limit, patterns, Order::Flush).await,
        SubCommand::Trigger {
            socket,
            limit,
            patterns,
        } => request(socket, Format::JSON, Some(limit), patterns, Order::Trigger).await,
        SubCommand::Rotate { socket } => {
            request(socket, Format::JSON, None, vec![], Order::Rotate).await
        }
        SubCommand::TestRegex {
            config,
            regex,
            line,
        } => test_regex(config, regex, line),
        SubCommand::TestFilter {
            config,
            stream_filter,
            line,
        } => test_filter(config, stream_filter, line),
        SubCommand::TestConfig {
            config,
            format,
            verbose,
        } => test_config(config, format, verbose),
    };
    match result {
        Ok(()) => {
            exit(0);
        }
        Err(err) => {
            eprintln!("ERROR {err}");
            exit(1);
        }
    }
}
