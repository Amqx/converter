pub mod lang;
pub mod matcher;
pub mod matching;
pub mod types;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExtensionError {
    #[error("Unknown language codepoint found: {0}")]
    UnknownLanguageCode(String),
    /// A repository that fetches its index over the network couldn't get at it.
    #[error("could not reach the {repo} extension index: {message}")]
    IndexUnreachable { repo: String, message: String },
    /// We reached the index and it wasn't what we expected - usually the repo
    /// changed its schema out from under us.
    #[error("could not parse the {repo} extension index: {message}")]
    MalformedIndex { repo: String, message: String },
    #[error("Internal error: {0}")]
    Internal(String),
}

pub type ExtResult<T> = Result<T, ExtensionError>;
