use super::{FailedExtension, ReadError, SkippedExtension};
use crate::ReaderID;
use crate::ir::types::Backup;

#[derive(Debug, PartialEq, Clone, Default)]
pub struct ReadReport {
    pub failed: Vec<FailedExtension>,
    pub skipped: Vec<SkippedExtension>,
}

pub trait BackupReader {
    fn reader(&self) -> ReaderID;
    fn read_backup(&self, bytes: &[u8]) -> Result<(Backup, ReadReport), ReadError>;
}
