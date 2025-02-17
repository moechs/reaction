use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use thiserror::Error;
use tokio_util::codec::{Decoder, LengthDelimitedCodec};

#[derive(Error, Debug)]
pub enum DecodeError {
    #[error("{0}")]
    Io(#[from] tokio::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

pub struct LengthPrefixedBincode<T: DeserializeOwned> {
    inner: LengthDelimitedCodec,
    _marker: PhantomData<T>,
}

impl<T: DeserializeOwned> LengthPrefixedBincode<T> {
    pub fn new() -> Self {
        LengthPrefixedBincode {
            inner: LengthDelimitedCodec::new(),
            _marker: PhantomData,
        }
    }
}

impl<T: DeserializeOwned> Default for LengthPrefixedBincode<T> {
    fn default() -> Self {
        Self::new()
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
            Ok(Some(data)) => match serde_json::from_slice(&data) {
                Ok(thing) => Ok(Some(thing)),
                Err(err) => Err(err.into()),
            },
            Ok(None) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }
}
