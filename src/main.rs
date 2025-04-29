use std::{io::IsTerminal, process::exit};

use clap::Parser;
use reaction::{
    cli::{Cli, SubCommand},
    client::{request, test_regex},
    daemon::daemon,
    protocol::Order,
};
use tracing::{error, Level};

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

    let (is_daemon, level) = if let SubCommand::Start { loglevel, .. } = cli.command {
        (true, loglevel)
    } else {
        (false, Level::DEBUG)
    };

    if is_daemon {
        // Set log level
        if let Err(err) = tracing_subscriber::fmt::fmt()
            .without_time()
            .with_target(false)
            .with_ansi(std::io::stdout().is_terminal())
            .with_max_level(level)
            // .with_max_level(Level::TRACE)
            .try_init()
        {
            eprintln!("ERROR could not initialize logging: {err}");
            exit(1);
        }
    }

    let result = match cli.command {
        SubCommand::Start {
            config,
            socket,
            ..
        } => daemon(config, socket).await,
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
        SubCommand::TestRegex {
            config,
            regex,
            line,
        } => test_regex(config, regex, line),
    };
    match result {
        Ok(()) => {
            exit(0);
        }
        Err(err) => {
            if is_daemon {
                error!("{err}");
            } else {
                eprintln!("ERROR {err}");
            }
            exit(1);
        }
    }
}
