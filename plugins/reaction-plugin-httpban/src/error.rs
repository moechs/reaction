use pingora_core::{BError, Error, ErrorSource, ErrorType, RetryType};

pub fn new_bad_setup<S>(message: S) -> BError
where
    S: Into<String>,
{
    pingora_core::Error {
        etype: ErrorType::InvalidHTTPHeader,
        esource: ErrorSource::Downstream,
        retry: RetryType::Decided(false),
        cause: Some(message.into().into()),
        context: None,
    }
    .into()
}

#[derive(Debug)]
pub enum HttpBanError {
    MalformedRequest(String),
    BadSetup(BError),
}

// impl HttpBanError {
//     pub fn new_bad_setup<S>(message: S) -> HttpBanError
//     where
//         S: Into<String>,
//     {
//         Self::BadSetup(new_bad_setup(message))
//     }
// }

impl std::error::Error for HttpBanError {}

impl std::fmt::Display for HttpBanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpBanError::MalformedRequest(e) => write!(f, "Malformed Request: {e}"),
            HttpBanError::BadSetup(e) => write!(f, "Bad setup: {e}"),
        }
    }
}

impl From<Error> for HttpBanError {
    fn from(value: Error) -> Self {
        HttpBanError::BadSetup(value.into())
    }
}
impl From<BError> for HttpBanError {
    fn from(value: BError) -> Self {
        HttpBanError::BadSetup(value.into())
    }
}

impl From<String> for HttpBanError {
    fn from(value: String) -> Self {
        HttpBanError::MalformedRequest(value)
    }
}
impl From<&str> for HttpBanError {
    fn from(value: &str) -> Self {
        HttpBanError::MalformedRequest(value.into())
    }
}
