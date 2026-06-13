use super::{
    Result, Backup, BackupReader, BackupWriter,
};
use crate::matching::Matcher;

pub struct TachiyomiConverter;

#[allow(dead_code)]
mod proto {
    include!(concat!(env!("OUT_DIR"), "/tachiyomisy.backup.rs"));
}

impl BackupReader for TachiyomiConverter {
    fn read_backup(&self, bytes: &[u8]) -> Result<Backup> {
        todo!()
    }
}

impl BackupWriter for TachiyomiConverter {
    fn write_backup(&self, backup: &Backup, matcher: &Matcher) -> Result<Vec<u8>> {
        todo!()
    }
}