use std::sync::Arc;

use crate::ReaderID;
use crate::extensions::types::Extension;

#[derive(Debug, PartialEq, Clone)]
pub struct Backup {
    pub reader: ReaderID,
    pub version: Option<String>,
    pub extensions: Vec<Arc<Extension>>,
    pub entries: Vec<Manga>,
    pub categories: Vec<Category>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct Manga {
    // Mandatory fields
    pub id: String,
    pub title: String,
    /// Shared with the matching entry in `Backup::extensions`.
    pub extension: Arc<Extension>,
    pub status: MangaStatus,
    pub nsfw: bool,
    pub favorite: bool,

    // Optional fields
    pub author: Option<String>,
    pub artist: Option<String>,
    pub description: Option<String>,
    pub chapters: Vec<Chapter>,
    /// Only for chapters missing from `chapters` above. If the chapter is
    /// there, its read date goes on `Chapter::last_read` instead.
    pub history: Vec<History>,
    pub tags: Vec<String>,
    pub categories: Vec<CategoryID>,
    pub cover_url: Option<String>,
    pub date_added: Option<jiff::Timestamp>,
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct History {
    // Which chapter this was: some backups keep an id, others only the URL.
    pub chapter_id: Option<String>,
    pub url: Option<String>,
    pub last_read: jiff::Timestamp,
}

#[derive(Debug, PartialEq, Eq, Clone, Hash, Default)]
pub enum MangaStatus {
    #[default]
    Unknown,
    Ongoing,
    Completed,
    Licensed,
    PublishingFinished,
    Cancelled,
    OnHiatus,
}

#[derive(Debug, PartialEq, Clone)]
pub struct Chapter {
    // Mandatory fields
    pub id: String,
    pub title: String,
    pub read: bool,
    pub last_page_read: i32,

    // Optional fields.
    pub scanlator: Option<String>,
    pub date_fetched: Option<jiff::Timestamp>,
    pub date_uploaded: Option<jiff::Timestamp>,
    pub chapter_number: Option<f64>,
    pub last_modified: Option<jiff::Timestamp>,
    pub last_read: Option<jiff::Timestamp>,
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct CategoryID(pub String);

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct Category {
    pub name: String,
    pub id: CategoryID,
    pub last_modified: jiff::Timestamp,
}
