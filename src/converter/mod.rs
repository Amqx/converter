use crate::matching::{Matcher, SourceData};
use crate::reader::ReaderId;
use crate::report::{ReadReport, WriteReport};

pub mod aidoku;
pub mod tachiyomi;

pub struct Backup {
    pub reader: ReaderId,
    pub manga: Vec<Manga>,
    pub categories: Vec<Category>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackupStats {
    pub manga: usize,
    pub categories: usize,
    pub chapters: usize,
    pub history_entries: usize,
}

impl BackupStats {
    pub fn from_backup(backup: &Backup) -> Self {
        Self {
            manga: backup.manga.len(),
            categories: backup.categories.len(),
            chapters: backup.manga.iter().map(|manga| manga.chapters.len()).sum(),
            history_entries: backup.manga.iter().map(|manga| manga.history.len()).sum(),
        }
    }
}

#[derive(PartialEq, Debug)]
pub struct Manga {
    pub id: String,
    pub source: SourceData,
    pub title: String,
    pub author: Option<String>,
    pub artist: Option<String>,
    pub desc: Option<String>,
    pub tags: Vec<String>,
    pub cover_url: Option<String>,
    pub status: MangaStatus,
    pub nsfw: bool,
    pub date_added: Option<i64>,
    pub categories: Vec<String>,
    pub chapters: Vec<Chapter>,
    pub history: Vec<History>,
    pub favorite: bool,
}

#[derive(Eq, PartialEq, Default, Debug)]
pub struct History {
    pub url: String,
    pub last_read: i64,
}

#[derive(Eq, PartialEq, Default, Debug)]
pub enum MangaStatus {
    #[default]
    Unknown = 0,
    Ongoing = 1,
    Completed = 2,
    Licensed = 3,
    PublishingFinished = 4,
    Cancelled = 5,
    OnHiatus = 6,
}

#[derive(PartialEq, Default, Debug)]
pub struct Chapter {
    pub id: String,
    pub title: String,
    pub scanlator: String,
    pub date_fetched: i64,
    pub date_uploaded: i64,
    pub chapter_number: f64,
    pub last_modified: i64,
    pub read: bool,
    pub last_page_read: i32,
}

#[derive(Eq, PartialEq, Default, Debug)]
pub struct Category {
    pub name: String,
    pub id: String,
    pub last_modified: i64,
}

#[derive(thiserror::Error, Debug)]
pub enum IRError {
    #[error("failed to parse backup: {0}")]
    Parse(String),

    #[error("failed to serialize backup: {0}")]
    Serialize(String),

    #[error("failed to decompress backup")]
    Decompress(#[from] std::io::Error),

    #[error("protobuf decode failed: {0}")]
    ProtobufDecode(#[from] prost::DecodeError),

    #[error("protobuf encode failed: {0}")]
    ProtobufEncode(#[from] prost::EncodeError),

    #[error("couldn't find matcher sources for: {0}")]
    Matcher(String),
}
pub type Result<T> = std::result::Result<T, IRError>;

pub trait BackupReader {
    fn read_backup(bytes: &[u8]) -> Result<(Backup, ReadReport)>;
}

pub trait BackupWriter {
    fn write_backup(backup: &Backup, matcher: &Matcher) -> Result<(Vec<u8>, WriteReport)>;
}
