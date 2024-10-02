mod action;
mod config;
mod filter;
mod messages;
mod pattern;
mod socket_messages;
mod stream;

use std::{collections::BTreeSet, fmt::Display, sync::Arc};

pub use action::Action;
pub use config::{Config, Patterns};
pub use filter::Filter;
pub use messages::*;
pub use pattern::Pattern;
pub use socket_messages::*;
pub use stream::Stream;

pub trait ActionFilter: Clone + Display + PartialEq + Eq + PartialOrd + Ord {
    fn patterns(&self) -> &BTreeSet<Arc<Pattern>>;
    fn full_name(&self) -> (&str, &str, &str);
}
