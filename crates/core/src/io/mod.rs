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

/// Rows a writer left out of an otherwise good backup.
///
/// The extension resolved and the entries behind it are the app's own; what the
/// target app spells differently is the part the writer could not produce. One
/// row per reason rather than per entry - a source whose ids a writer cannot
/// translate has the same thing to say about every entry under it.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DroppedEntries {
    pub reason: String,
    pub extension: Extension,
    pub count: usize,
    /// A few of the values responsible, for a report that has to stay short.
    pub examples: Vec<String>,
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
