use converter_core::ReaderID;
use converter_core::io::ReadError;
use converter_core::io::reader::{BackupReader, ReadReport};
use converter_core::ir::types::Backup;

pub struct AidokuReader {}

impl AidokuReader {
    pub fn new() -> Self {
        todo!()
    }
}

impl BackupReader for AidokuReader {
    fn reader(&self) -> ReaderID {
        todo!()
    }

    fn read_backup(&self, bytes: &[u8]) -> Result<(Backup, ReadReport), ReadError> {
        todo!()
    }
}
