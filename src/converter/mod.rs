use crate::matching::{Matcher, SourceData};
use crate::reader::ReaderId;
use crate::report::{ReadReport, SkippedManga, SkippedSource, SourceMapping, WriteReport};
use std::collections::{HashMap, HashSet};

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

#[derive(Clone, Copy, Eq, PartialEq, Default, Debug)]
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

impl MangaStatus {
    /// Both Aidoku and Tachiyomi encode status with the same numeric codes.
    pub fn from_code(code: i64) -> Self {
        match code {
            1 => Self::Ongoing,
            2 => Self::Completed,
            3 => Self::Licensed,
            4 => Self::PublishingFinished,
            5 => Self::Cancelled,
            6 => Self::OnHiatus,
            _ => Self::Unknown,
        }
    }

    pub fn code(self) -> i64 {
        self as i64
    }
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

/// Identity of a source backing a manga, used to dedupe mappings and to group
/// every manga that shares an unmatchable source into a single skip entry.
type SourceKey = (ReaderId, Option<String>, Option<String>);

fn source_key(source: &SourceData) -> SourceKey {
    (source.reader, source.external_id.clone(), source.name.clone())
}

fn source_label(source: &SourceData) -> String {
    format!(
        "{} ({})",
        source.name.as_deref().unwrap_or("unknown"),
        source.reader
    )
}

fn skipped_manga(manga: &Manga) -> SkippedManga {
    SkippedManga {
        title: manga.title.clone(),
        chapters: manga.chapters.len(),
        history: manga.history.len(),
    }
}

/// Shared per-source bookkeeping for [`BackupWriter`] implementations.
///
/// When a manga's source can't be matched to the target reader, that manga and
/// every later one sharing the source are skipped and folded into a single
/// [`SkippedSource`] entry. Mappings between matched sources are likewise
/// recorded only once per source.
#[derive(Default)]
pub struct WriteTracker {
    failed: HashSet<SourceKey>,
    skipped: Vec<SkippedSource>,
    skipped_index: HashMap<SourceKey, usize>,
    mapped: HashSet<SourceKey>,
    mappings: Vec<SourceMapping>,
}

impl WriteTracker {
    /// If `manga`'s source already failed to match, fold `manga` into that
    /// source's skip entry and return `true` so the caller can skip it.
    pub fn note_if_failed(&mut self, manga: &Manga) -> bool {
        let key = source_key(&manga.source);
        if !self.failed.contains(&key) {
            return false;
        }
        if let Some(&idx) = self.skipped_index.get(&key) {
            self.skipped[idx].manga.push(skipped_manga(manga));
        }
        true
    }

    /// Record the first failure for `manga`'s source, seeding its skip entry.
    pub fn record_failure(&mut self, manga: &Manga, reason: &str) {
        let key = source_key(&manga.source);
        let idx = self.skipped.len();
        self.skipped.push(SkippedSource {
            source_label: source_label(&manga.source),
            reason: reason.to_string(),
            manga: vec![skipped_manga(manga)],
        });
        self.skipped_index.insert(key.clone(), idx);
        self.failed.insert(key);
    }

    /// Record a `from -> to` source mapping the first time a source is converted.
    /// `to` is only computed when the mapping is new.
    pub fn record_mapping(&mut self, manga: &Manga, to: impl FnOnce() -> String) {
        if self.mapped.insert(source_key(&manga.source)) {
            self.mappings.push(SourceMapping {
                from: source_label(&manga.source),
                to: to(),
            });
        }
    }

    pub fn skipped_manga_count(&self) -> usize {
        self.skipped.iter().map(|source| source.manga.len()).sum()
    }

    pub fn into_parts(self) -> (Vec<SourceMapping>, Vec<SkippedSource>) {
        (self.mappings, self.skipped)
    }
}
