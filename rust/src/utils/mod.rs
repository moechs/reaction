pub mod cli;
pub mod logger;
mod parse_duration;
mod threadpool;

use bincode::Options;
pub use logger::SimpleLogger;
pub use parse_duration::parse_duration;
pub use threadpool::ThreadPool;

pub type BincodeOptions = bincode::config::WithOtherIntEncoding<
    bincode::config::DefaultOptions,
    bincode::config::VarintEncoding,
>;
pub fn bincode_options() -> BincodeOptions {
    bincode::DefaultOptions::new().with_varint_encoding()
}
