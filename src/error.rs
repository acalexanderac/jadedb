use std::io;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("corruption: {0}")]
    Corruption(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("database closed")]
    Closed,
}
