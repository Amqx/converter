use super::{FailedExtension, ReadError, SkippedExtension};
use crate::ReaderID;
use crate::ir::types::Backup;

#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub struct ReadReport {
    pub failed: Vec<FailedExtension>,
    pub skipped: Vec<SkippedExtension>,
}

/// `Send` so an [`Orchestrator`](crate::orchestrator::Orchestrator) holding one
/// can still be driven from an async task; see
/// [`ExtensionRepository`](crate::extensions::types::ExtensionRepository).
pub trait BackupReader: Send {
    fn reader(&self) -> ReaderID;
    fn read_backup(&self, bytes: &[u8]) -> Result<(Backup, ReadReport), ReadError>;
}
