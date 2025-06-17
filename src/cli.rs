use std::{fmt, path::PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use regex::Regex;
use tracing::Level;

#[derive(Parser)]
#[clap(version)]
#[command(
    name = "reaction",
    about = "Scan logs and take action",
    long_about = "A daemon that scans program outputs for repeated patterns, and takes action.
Aims at being more versatile and flexible than fail2ban, while being faster and having simpler configuration.

See usage examples, service configurations and good practices
on the wiki: https://reaction.ppom.me
"
)]
pub struct Cli {
    #[clap(subcommand)]
    pub command: SubCommand,
}

#[derive(Subcommand)]
pub enum SubCommand {
    /// Start reaction daemon
    Start {
        /// configuration file in json, jsonnet or yaml format, or directory containing those files. required.
        #[clap(short = 'c', long)]
        config: PathBuf,

        /// minimum log level to show
        #[clap(short = 'l', long, value_parser = parse_log_level, default_value_t = Level::INFO, ignore_case = true)]
        loglevel: Level,

        /// path to the client-daemon communication socket
        #[clap(short = 's', long, default_value = "/run/reaction/reaction.sock")]
        socket: PathBuf,
    },

    /// Show current matches and actions
    #[command(
        long_about = "Show current matches and which actions are still to be run (e.g. know what is currently banned"
    )]
    Show {
        /// path to the client-daemon communication socket
        #[clap(short = 's', long, default_value = "/run/reaction/reaction.sock")]
        socket: PathBuf,

        /// how to format output
        #[clap(short = 'f', long, default_value_t = Format::YAML)]
        format: Format,

        /// only show items related to this STREAM[.FILTER]
        #[clap(short = 'l', long, value_name = "STREAM[.FILTER]")]
        limit: Option<String>,

        /// only show items matching name=PATTERN regex
        #[clap(value_parser = parse_named_regex, value_name = "NAME=PATTERN")]
        patterns: Vec<(String, String)>,
    },

    /// Remove a target from reaction (e.g. unban)
    #[command(
        long_about = "Remove currently active matches and run currently pending actions for the specified TARGET. (e.g. unban)
Then prints the flushed matches and actions."
    )]
    Flush {
        /// path to the client-daemon communication socket
        #[clap(short = 's', long, default_value = "/run/reaction/reaction.sock")]
        socket: PathBuf,

        /// how to format output
        #[clap(short = 'f', long, default_value_t = Format::YAML)]
        format: Format,

        /// only show items related to this STREAM[.FILTER]
        #[clap(short = 'l', long, value_name = "STREAM[.FILTER]")]
        limit: Option<String>,

        /// only show items matching name=PATTERN regex
        #[clap(value_parser = parse_named_regex, value_name = "NAME=PATTERN")]
        patterns: Vec<(String, String)>,
    },

    /// Trigger a target in reaction (e.g. ban)
    #[command(
        long_about = "Trigger actions and remove currently active matches for the specified PATTERNS in the specified STREAM.FILTER. (e.g. ban)"
    )]
    Trigger {
        /// path to the client-daemon communication socket
        #[clap(short = 's', long, default_value = "/run/reaction/reaction.sock")]
        socket: PathBuf,

        /// STREAM.FILTER to trigger
        #[clap(value_name = "STREAM.FILTER")]
        limit: Option<String>,

        /// PATTERNs to trigger on (e.g. ip=1.2.3.4)
        #[clap(value_parser = parse_named_regex, value_name = "NAME=PATTERN")]
        patterns: Vec<(String, String)>,
    },

    /// Test a regex
    #[command(
        name = "test-regex",
        long_about = "Test a REGEX against one LINE, or against standard input.
Giving a configuration file permits to use its patterns in REGEX."
    )]
    TestRegex {
        /// configuration file in json, jsonnet or yaml format, or directory containing those files. required.
        #[clap(short = 'c', long)]
        config: PathBuf,

        /// Regex to test
        #[clap(value_name = "REGEX")]
        regex: String,

        /// Line to be tested
        #[clap(value_name = "LINE")]
        line: Option<String>,
    },
    /// Test your configuration
    TestConfig {
        /// either a configuration file in json, jsonnet or yaml format, or a directory containing those files. required.
        #[clap(short = 'c', long)]
        config: PathBuf,

        /// how to format output
        #[clap(short = 'f', long, default_value_t = Format::YAML)]
        format: Format,

        /// whether to output additional information
        #[clap(short = 'v', long, default_value_t = false)]
        verbose: bool,
    },
}

// Enums

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, ValueEnum)]
pub enum Format {
    JSON,
    YAML,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Format::JSON => write!(f, "json"),
            Format::YAML => write!(f, "yaml"),
        }
    }
}

fn parse_named_regex(s: &str) -> Result<(String, String), String> {
    let (name, v) = s
        .split_once('=')
        .ok_or("When given as a positional argument, a pattern must be prefixed with a name, ex: ip=192.168.0.1")?;
    let _ = Regex::new(v).map_err(|err| format!("{}", err))?;
    Ok((name.to_string(), v.to_string()))
}

fn parse_log_level(s: &str) -> Result<Level, String> {
    match s.to_ascii_uppercase().as_str() {
        "DEBUG" => Ok(Level::DEBUG),
        "INFO" => Ok(Level::INFO),
        "WARN" => Ok(Level::WARN),
        "ERROR" => Ok(Level::ERROR),
        "FATAL" => Ok(Level::ERROR),
        _ => Err("must be one of ERROR, WARN, INFO, DEBUG".into()),
    }
}
