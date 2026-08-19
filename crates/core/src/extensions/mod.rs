pub mod matcher;
pub mod lang;
pub mod types;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExtensionError {
    #[error("Unknown language codepoint found: {0}")]
    UnknownLanguageCode(String),
}

pub type ExtResult<T> = Result<T, ExtensionError>;