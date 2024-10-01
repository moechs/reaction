mod action;
mod config;
mod filter;
mod messages;
mod pattern;
mod socket_messages;
mod stream;

use std::{collections::BTreeSet, fmt::Display, sync::Arc};

pub use action::*;
pub use config::*;
pub use filter::*;
pub use messages::*;
pub use pattern::*;
pub use socket_messages::*;
pub use stream::*;

pub trait ActionFilter: Clone + Display + PartialEq + Eq + PartialOrd + Ord {
    fn patterns(&self) -> &BTreeSet<Arc<Pattern>>;
    fn full_name<'a>(&'a self) -> (&'a str, &'a str, &'a str);
}
