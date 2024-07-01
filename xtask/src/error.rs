use std::fmt::{self, Display, Formatter};

pub(crate) type TaskResult<T> = Result<T, SimpleError>;

#[derive(Debug)]
pub(crate) struct SimpleError(String);

impl Display for SimpleError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SimpleError {}

impl From<&str> for SimpleError {
    fn from(err: &str) -> Self {
        Self(err.into())
    }
}

impl From<String> for SimpleError {
    fn from(err: String) -> Self {
        Self(err)
    }
}

impl From<std::io::Error> for SimpleError {
    fn from(value: std::io::Error) -> Self {
        Self(format!("IO error: {}", value))
    }
}

impl From<pico_args::Error> for SimpleError {
    fn from(value: pico_args::Error) -> Self {
        Self(format!("CLI argument error: {}", value))
    }
}
