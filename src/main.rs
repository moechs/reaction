use std::{io::IsTerminal, process::exit};

use clap::Parser;
use reaction::{
    cli::{Cli, Format, SubCommand},
    client::{request, test_config, test_regex},
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

    if let SubCommand::Start {
        loglevel,
        config,
        socket,
    } = cli.command
    {
        if let Err(err) = tracing_subscriber::fmt::fmt()
            .without_time()
            .with_target(false)
            .with_ansi(std::io::stdout().is_terminal())
            .with_max_level(loglevel)
            // .with_max_level(Level::TRACE)
            .try_init()
        {
            eprintln!("ERROR could not initialize logging: {err}");
            exit(1);
        }
        daemon(config, socket).await;
    } else {
        let result = match cli.command {
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
            SubCommand::TestRegex {
                config,
                regex,
                line,
            } => test_regex(config, regex, line),
            SubCommand::TestConfig {
                config,
                format,
                verbose,
            } => test_config(config, format, verbose),
            // Can't be daemon
            _ => Ok(()),
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
}
