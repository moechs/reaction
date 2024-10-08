pub mod cli;
pub mod logger;
mod parse_duration;

use std::marker::PhantomData;

use bincode::Options;
pub use logger::SimpleLogger;
pub use parse_duration::parse_duration;
use serde::de::DeserializeOwned;
use thiserror::Error;
use tokio_util::codec::{Decoder, LengthDelimitedCodec};

pub type BincodeOptions = bincode::config::WithOtherIntEncoding<
    bincode::config::DefaultOptions,
    bincode::config::VarintEncoding,
>;
pub fn bincode_options() -> BincodeOptions {
    bincode::DefaultOptions::new().with_varint_encoding()
}

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("{0}")]
    Io(#[from] tokio::io::Error),
    #[error("{0}")]
    Bincode(#[from] bincode::Error),
}

pub struct LengthPrefixedBincode<T: DeserializeOwned> {
    inner: LengthDelimitedCodec,
    bin: BincodeOptions,
    _marker: PhantomData<T>,
}

impl<T: DeserializeOwned> LengthPrefixedBincode<T> {
    pub fn new() -> Self {
        LengthPrefixedBincode {
            inner: LengthDelimitedCodec::new(),
            bin: bincode_options(),
            _marker: PhantomData,
        }
    }
}

impl<T: DeserializeOwned> Decoder for LengthPrefixedBincode<T> {
    type Item = T;
    type Error = DecodeError;

    fn decode(
        &mut self,
        src: &mut tokio_util::bytes::BytesMut,
    ) -> Result<Option<Self::Item>, Self::Error> {
        match self.inner.decode(src) {
            Ok(Some(data)) => match self.bin.deserialize(&data) {
                Ok(thing) => Ok(Some(thing)),
                Err(err) => Err(err.into()),
            },
            Ok(None) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }
}
