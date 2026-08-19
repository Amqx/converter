#[macro_use]
mod macros;

pub mod extensions;
pub mod format;
pub mod io;
pub mod ir;
pub mod orchestrator;

use std::fmt::Debug;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("core/extensions error: {0}")]
    Extensions(#[from] extensions::ExtensionError),
    #[error("core/ir error: {0}")]
    IR(#[from] ir::IRError),
    #[error("core/orchestrator error: {0}")]
    Orchestrator(#[from] orchestrator::OrchestratorError),
    #[error("core/reader error: {0}")]
    BackupReader(#[from] io::ReadError),
    #[error("core/writer error: {0}")]
    BackupWriter(#[from] io::WriteError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReaderID(pub &'static str);

impl std::fmt::Display for ReaderID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}