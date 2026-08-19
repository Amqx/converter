use super::{FailedExtension, SkippedExtension, WriteError};
use crate::ReaderID;
use crate::ir::types::Backup;
use crate::orchestrator::mapping::ExtensionMapping;

#[derive(Debug, PartialEq, Clone, Default)]
pub struct WriteReport {
    pub failed: Vec<FailedExtension>,
    pub skipped: Vec<SkippedExtension>,
}

pub trait BackupWriter {
    fn reader(&self) -> ReaderID;

    /// `mapping` says which target extension each of the backup's extensions
    /// became.
    fn write_backup(
        &self,
        backup: &Backup,
        mapping: &ExtensionMapping,
    ) -> Result<(Vec<u8>, WriteReport), WriteError>;
}
