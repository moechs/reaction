mod env;
mod map;

pub use map::Tree;
pub use env::Database;

use serde::{de::DeserializeOwned, Serialize};

pub trait KeyType: Ord + Serialize + DeserializeOwned + Clone {}
pub trait ValueType: Ord + Serialize + DeserializeOwned + Clone {}

impl<T> KeyType for T where T: Ord + Serialize + DeserializeOwned + Clone {}
impl<T> ValueType for T where T: Ord + Serialize + DeserializeOwned + Clone {}
