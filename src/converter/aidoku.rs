use super::{Backup, BackupReader, BackupWriter, Result};
use crate::matching::Matcher;

pub struct AidokuConverter;

impl BackupReader for AidokuConverter {
    fn read_backup(&self, bytes: &[u8]) -> Result<Backup> {
        todo!()
    }
}

impl BackupWriter for AidokuConverter {
    fn write_backup(&self, backup: &Backup, matcher: &Matcher) -> Result<Vec<u8>> {
        todo!()
    }
}
