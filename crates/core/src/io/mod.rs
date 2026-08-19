pub mod reader;
pub mod writer;

use crate::extensions::types::Extension;
use thiserror::Error;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SkippedExtension {
    pub reason: String,
    pub extension: Extension,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FailedExtension {
    pub reason: String,
    pub extension: Extension,
}

#[derive(Error, Debug)]
pub enum ReadError {
    /// The container would not open: wrong file type, truncated, or corrupt.
    #[error("could not decompress the backup: {0}")]
    Decompress(String),
    /// The bytes came out of the container but are not this app's backup format.
    #[error("could not parse the backup: {0}")]
    Malformed(String),
}

#[derive(Error, Debug)]
pub enum WriteError {
    /// The backup was assembled but the app's container would not take it.
    #[error("could not encode the backup: {0}")]
    Encode(String),
}
