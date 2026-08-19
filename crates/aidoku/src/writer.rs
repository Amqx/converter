use converter_core::ReaderID;
use converter_core::io::WriteError;
use converter_core::io::writer::{BackupWriter, WriteReport};
use converter_core::ir::types::Backup;
use converter_core::orchestrator::mapping::ExtensionMapping;

pub struct AidokuWriter {}

impl AidokuWriter {
    pub fn new() -> Self {
        todo!()
    }
}

impl BackupWriter for AidokuWriter {
    fn reader(&self) -> ReaderID {
        todo!()
    }

    fn write_backup(
        &self,
        backup: &Backup,
        mapping: &ExtensionMapping,
    ) -> Result<(Vec<u8>, WriteReport), WriteError> {
        todo!()
    }
}
