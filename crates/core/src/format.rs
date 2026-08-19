use crate::ReaderID;
use crate::extensions::types::ExtensionRepository;
use crate::io::reader::BackupReader;
use crate::io::writer::BackupWriter;

/// Everything a single app plugs into the pipeline.
pub trait BackupFormat {
    fn id() -> ReaderID;

    fn reader() -> Box<dyn BackupReader>;

    /// `None` for formats we can only import from.
    fn writer() -> Option<Box<dyn BackupWriter>>;

    /// Repositories to search when this format is the conversion target.
    fn repositories() -> Vec<Box<dyn ExtensionRepository>>;
}
