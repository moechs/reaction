mod action;
mod config;
mod filter;
mod parse_duration;
mod pattern;
mod stream;

pub use action::Action;
pub use config::{Config, Patterns};
pub use filter::Filter;
use parse_duration::parse_duration;
pub use pattern::Pattern;
pub use stream::Stream;

use chrono::{DateTime, Local};

pub type Time = DateTime<Local>;
pub type Match = Vec<String>;
