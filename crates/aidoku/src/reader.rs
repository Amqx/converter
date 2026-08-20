use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use converter_core::extensions::lang::Language;
use converter_core::extensions::types::{Extension, ExtensionRepositoryInfo};
use converter_core::io::reader::{BackupReader, ReadReport};
use converter_core::io::{ReadError, SkippedExtension};
use converter_core::ir::types::{
    Backup, Category, CategoryID, Chapter, History, Manga, MangaStatus,
};
use jiff::{SignedDuration, Timestamp};
use serde::Deserialize;
use serde::de::{Deserializer, Visitor};

use crate::Aidoku;

/// Series the user side-loaded as files.
const LOCAL_SOURCE_ID: &str = "local";

/// `MangaObject.nsfw` is a rating rather than a flag; 2 and up is what the IR's boolean means.
const NSFW_RATING: i64 = 2;

/// Keys an entry, a chapter and a history row all agree on: Aidoku scopes every
/// id to the source that produced it, and `BackupManager` restores by the pair.
type MangaKey = (String, String);

pub struct AidokuReader {}

impl AidokuReader {
    pub const fn new() -> Self {
        Self {}
    }
}

impl Default for AidokuReader {
    fn default() -> Self {
        Self::new()
    }
}

impl BackupReader for AidokuReader {
    converter_core::format_id!(Aidoku);

    fn read_backup(&self, bytes: &[u8]) -> Result<(Backup, ReadReport), ReadError> {
        Ok(assemble(parse(bytes)?))
    }
}

/// Aidoku writes a binary property list and `Backup.load` falls back to JSON,
/// so a backup old enough to predate the plist still has to read here.
fn parse(bytes: &[u8]) -> Result<RawBackup, ReadError> {
    let plist_error = match read_plist(bytes) {
        Ok(backup) => return Ok(backup),
        // It opened as a property list, so JSON is not what this file is.
        Err(PlistFailure::UnrepresentableDate) => {
            return Err(ReadError::Malformed(
                "the property list holds a date outside the range a date can \
                 represent; a source that recorded milliseconds where seconds \
                 were expected writes one"
                    .to_string(),
            ));
        }
        Err(PlistFailure::NotAPlist(error)) => error,
    };

    serde_json::from_slice(bytes).map_err(|json_error| {
        // Which of the two the file was meant to be is the user's question to
        // answer, so say what both parsers made of it.
        ReadError::Malformed(format!(
            "not a property list ({plist_error}) or JSON ({json_error})"
        ))
    })
}

enum PlistFailure {
    /// `plist` routes every `Event::Date` through `Date::to_xml_format()`,
    /// which panics on a date outside the year 0000-9999 window rather than
    /// returning an error the deserializer could surface. Reaching a date at
    /// all means the container opened, so this is a bad value in a real
    /// property list, not a file that was never one.
    UnrepresentableDate,
    NotAPlist(plist::Error),
}

/// Deserializes a property list, turning `plist`'s date panic into a value.
///
/// The panic hook is process-wide, so silencing it here also silences a panic
/// another thread happens to hit in the same window; that is the price of not
/// printing a backtrace at the user every time they open a corrupt backup.
/// `catch_unwind` also does nothing under `panic = "abort"`, which the
/// workspace does not set.
fn read_plist(bytes: &[u8]) -> Result<RawBackup, PlistFailure> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        plist::from_bytes::<RawBackup>(bytes)
    }));
    std::panic::set_hook(previous);

    match result {
        Ok(Ok(backup)) => Ok(backup),
        Ok(Err(error)) => Err(PlistFailure::NotAPlist(error)),
        Err(_) => Err(PlistFailure::UnrepresentableDate),
    }
}

fn assemble(raw: RawBackup) -> (Backup, ReadReport) {
    let mut sources = Sources::new(raw.sources);

    let mut chapters: HashMap<MangaKey, Vec<RawChapter>> = HashMap::new();
    for chapter in raw.chapters {
        chapters
            .entry((chapter.source_id.clone(), chapter.manga_id.clone()))
            .or_default()
            .push(chapter);
    }

    let mut history: HashMap<MangaKey, Vec<RawHistory>> = HashMap::new();
    for row in raw.history {
        history
            .entry((row.source_id.clone(), row.manga_id.clone()))
            .or_default()
            .push(row);
    }

    // `manga` holds every series the database knows, read-once ones included;
    // `library` holds the subset the user actually added, which is what the
    // IR's `favorite` means.
    let mut library: HashMap<MangaKey, RawLibraryManga> = HashMap::new();
    for entry in raw.library {
        library.insert((entry.source_id.clone(), entry.manga_id.clone()), entry);
    }

    let entries: Vec<Manga> = raw
        .manga
        .into_iter()
        .filter_map(|manga| {
            let extension = sources.resolve(&manga.source_id)?;
            let key = (manga.source_id.clone(), manga.id.clone());
            Some(entry(
                manga,
                extension,
                chapters.remove(&key).unwrap_or_default(),
                history.remove(&key).unwrap_or_default(),
                library.remove(&key),
            ))
        })
        .collect();

    let categories = raw.categories.into_iter().filter_map(category).collect();
    let (extensions, skipped) = sources.finish();

    let backup = Backup {
        reader: Aidoku::ID,
        version: non_empty(raw.version),
        extensions,
        entries,
        categories,
    };

    let report = ReadReport {
        failed: Vec::new(),
        skipped,
    };

    (backup, report)
}

/// Hands out exactly one [`Arc<Extension>`] per Aidoku source id.
struct Sources {
    /// Insertion order, so the IR doesn't reshuffle between runs the way a `HashMap` would.
    order: Vec<String>,
    by_id: HashMap<String, Arc<Extension>>,
    skipped: Vec<SkippedExtension>,
    reported: HashSet<String>,
}

impl Sources {
    fn new(listed: Vec<RawSource>) -> Self {
        let mut sources = Self {
            order: Vec::new(),
            by_id: HashMap::new(),
            skipped: Vec::new(),
            reported: HashSet::new(),
        };

        for source in listed {
            let id = source.into_id();
            if !id.is_empty() && synthetic_reason(&id).is_none() {
                sources.register(id);
            }
        }

        sources
    }

    /// `None` when the id has no installable source behind it, so the caller drops the entry.
    fn resolve(&mut self, id: &str) -> Option<Arc<Extension>> {
        if id.is_empty() {
            return None;
        }
        if let Some(reason) = synthetic_reason(id) {
            self.report(id, reason);
            return None;
        }
        Some(self.register(id.to_string()))
    }

    fn register(&mut self, id: String) -> Arc<Extension> {
        if let Some(existing) = self.by_id.get(&id) {
            return Arc::clone(existing);
        }

        let extension = Arc::new(extension_for(&id));
        self.order.push(id.clone());
        self.by_id.insert(id, Arc::clone(&extension));
        extension
    }

    fn report(&mut self, id: &str, reason: &str) {
        if !self.reported.insert(id.to_string()) {
            return;
        }
        self.skipped.push(SkippedExtension {
            reason: reason.to_string(),
            extension: extension_for(id),
        });
    }

    fn finish(self) -> (Vec<Arc<Extension>>, Vec<SkippedExtension>) {
        let Self {
            order,
            mut by_id,
            skipped,
            ..
        } = self;

        let extensions = order
            .into_iter()
            .filter_map(|id| by_id.remove(&id))
            .collect();

        (extensions, skipped)
    }
}

/// A backup names its sources by id and records nothing else about them.
fn extension_for(id: &str) -> Extension {
    let (name, lang) = describe(id);

    Extension {
        reader: Aidoku::ID,
        name,
        external_id: id.to_string(),
        url: String::new(),
        lang,
        repo: ExtensionRepositoryInfo::new(Aidoku::ID, None, None, None, None),
    }
}

/// Splits `en.asurascans` into "asurascans" and English.
fn describe(id: &str) -> (String, Language) {
    match id.split_once('.') {
        Some((prefix, site)) if !site.is_empty() => Language::from_code(prefix).map_or_else(
            |_| (id.to_string(), Language::Unknown),
            |lang| (site.to_string(), lang),
        ),
        _ => (id.to_string(), Language::Unknown),
    }
}

/// Sources the app synthesises rather than installs.
fn synthetic_reason(id: &str) -> Option<&'static str> {
    (id == LOCAL_SOURCE_ID).then_some("stored locally as files rather than fetched from a source")
}

fn entry(
    raw: RawManga,
    extension: Arc<Extension>,
    mut raw_chapters: Vec<RawChapter>,
    raw_history: Vec<RawHistory>,
    library: Option<RawLibraryManga>,
) -> Manga {
    raw_chapters.sort_by_key(|chapter| chapter.source_order);

    // Aidoku keeps no read flag on a chapter: `ChapterObject.history` is a
    // relation restored by matching `(sourceId, mangaId, chapterId)`, so the
    // join has to happen here for the IR to see a chapter as read at all.
    let mut read_at: HashMap<String, RawHistory> = HashMap::new();
    for row in raw_history {
        read_at.entry(row.chapter_id.clone()).or_insert(row);
    }

    let chapters: Vec<Chapter> = raw_chapters
        .into_iter()
        .map(|raw_chapter| {
            // Looked up rather than taken: a backup repeats a chapter row too,
            // and Aidoku hands every copy the same history object, so consuming
            // the row here would leave the second copy looking unread.
            let history = read_at.get(&raw_chapter.id);
            chapter(raw_chapter, history)
        })
        .collect();

    let covered: HashSet<&str> = chapters.iter().map(|chapter| chapter.id.as_str()).collect();
    let mut orphaned: Vec<History> = read_at
        .iter()
        .filter(|(id, _)| !covered.contains(id.as_str()))
        .filter_map(|(id, row)| {
            Some(History {
                chapter_id: non_empty(Some(id.clone())),
                // Aidoku's history rows key on the chapter's id; the URL is a
                // field of the chapter, which is the half that went missing.
                url: None,
                // A history row whose date we cannot represent carries nothing else, so drop it.
                last_read: row.date_read?.0?,
            })
        })
        .collect();
    // A `HashMap` hands its entries back in whatever order it likes, and the IR
    // is compared by value.
    orphaned.sort_by(|a, b| a.chapter_id.cmp(&b.chapter_id));

    let (favorite, categories, date_added) = library.map_or_else(
        || (false, Vec::new(), None),
        |library| {
            (
                true,
                library.categories.into_iter().map(CategoryID).collect(),
                library.date_added.and_then(|date| date.0),
            )
        },
    );

    Manga {
        id: raw.id,
        title: raw.title,
        extension,
        status: manga_status(raw.status),
        nsfw: raw.nsfw >= NSFW_RATING,
        favorite,
        author: non_empty(raw.author),
        artist: non_empty(raw.artist),
        description: non_empty(raw.desc),
        chapters,
        history: orphaned,
        tags: raw.tags,
        categories,
        cover_url: non_empty(raw.cover),
        date_added,
    }
}

fn chapter(raw: RawChapter, history: Option<&RawHistory>) -> Chapter {
    let RawChapter {
        id,
        title,
        scanlator,
        chapter,
        date_uploaded,
        ..
    } = raw;

    Chapter {
        id,
        title: title.unwrap_or_default(),
        read: history.is_some_and(|row| row.completed),
        last_page_read: history.map_or(0, |row| page_index(row.progress.unwrap_or_default())),
        scanlator: non_empty(scanlator),
        // Aidoku records neither when it fetched a chapter nor when it last
        // touched the row; `MangaObject.lastUpdated` is per entry, not per chapter.
        date_fetched: None,
        date_uploaded: date_uploaded.and_then(|date| date.0),
        chapter_number: chapter,
        last_modified: None,
        last_read: history
            .and_then(|row| row.date_read)
            .and_then(|date| date.0),
    }
}

/// `None` for a category with no title: a library entry names its categories by
/// title, so one without a title is a row nothing can reference.
fn category(raw: RawCategory) -> Option<Category> {
    let title = raw.into_title().filter(|title| !title.is_empty())?;

    Some(Category {
        id: CategoryID(title.clone()),
        name: title,
        last_modified: Timestamp::UNIX_EPOCH,
    })
}

/// Mirrors `PublishingStatus`, which the database stores raw. `notPublished`
/// has no IR counterpart and `toNew()` upstream folds it into `unknown`, which
/// is also what Aidoku does with any value the enum has since outgrown.
const fn manga_status(value: i64) -> MangaStatus {
    match value {
        1 => MangaStatus::Ongoing,
        2 => MangaStatus::Completed,
        3 => MangaStatus::Cancelled,
        4 => MangaStatus::OnHiatus,
        _ => MangaStatus::Unknown,
    }
}

/// A page index is an `Int` upstream and an `i32` in the IR. Clamp rather than
/// wrap, and read a negative as the start: Aidoku writes `-1` for a chapter it
/// tracked without ever recording a page.
fn page_index(value: i64) -> i32 {
    i32::try_from(value.clamp(0, i64::from(i32::MAX))).unwrap_or(i32::MAX)
}

/// Aidoku writes `""` about as often as it omits a field; the IR means "absent" by `None`.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawBackup {
    #[serde(default)]
    library: Vec<RawLibraryManga>,
    #[serde(default)]
    history: Vec<RawHistory>,
    #[serde(default)]
    manga: Vec<RawManga>,
    #[serde(default)]
    chapters: Vec<RawChapter>,
    #[serde(default)]
    categories: Vec<RawCategory>,
    #[serde(default)]
    sources: Vec<RawSource>,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawManga {
    id: String,
    source_id: String,
    title: String,
    author: Option<String>,
    artist: Option<String>,
    desc: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    cover: Option<String>,
    #[serde(default)]
    status: i64,
    #[serde(default)]
    nsfw: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawChapter {
    id: String,
    source_id: String,
    manga_id: String,
    title: Option<String>,
    scanlator: Option<String>,
    chapter: Option<f64>,
    date_uploaded: Option<BackupDate>,
    #[serde(default)]
    source_order: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHistory {
    source_id: String,
    manga_id: String,
    chapter_id: String,
    date_read: Option<BackupDate>,
    progress: Option<i64>,
    #[serde(default)]
    completed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLibraryManga {
    source_id: String,
    manga_id: String,
    date_added: Option<BackupDate>,
    #[serde(default)]
    categories: Vec<String>,
}

/// Both spellings `BackupSource`'s decoder accepts: a bare id, or an object
/// carrying the config of a source the user side-loaded.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawSource {
    Id(String),
    Object { id: String },
}

impl RawSource {
    fn into_id(self) -> String {
        match self {
            Self::Id(id) | Self::Object { id } => id,
        }
    }
}

/// Likewise `BackupCategory`: backups before the sort order existed are a plain
/// list of titles.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawCategory {
    Title(String),
    Object { title: Option<String> },
}

impl RawCategory {
    fn into_title(self) -> Option<String> {
        match self {
            Self::Title(title) => Some(title),
            Self::Object { title } => title,
        }
    }
}

/// A plist date reaches serde as the RFC 3339 string the `plist` crate renders
/// it to; the JSON backups older builds wrote spell the same instant as seconds
/// since the epoch, because `Backup.load` decodes those with
/// `.secondsSince1970`. Accept either, since the file itself might be either.
/// An unrepresentable value degrades to `None` rather than rejecting the whole
/// file: one bad date should not take down every entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BackupDate(Option<Timestamp>);

impl<'de> Deserialize<'de> for BackupDate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Every `visit_*` below answers `Ok`, so a value this cannot represent
        // is already a `None` by the time it gets here. What is left is the
        // document being broken rather than one date in it, and swallowing that
        // would resume the parser from a position it has lost track of.
        deserializer.deserialize_any(DateVisitor)
    }
}

struct DateVisitor;

impl Visitor<'_> for DateVisitor {
    type Value = BackupDate;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("an RFC 3339 date or a number of seconds since the epoch")
    }

    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
        Ok(BackupDate(Timestamp::from_second(value).ok()))
    }

    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(BackupDate(
            i64::try_from(value)
                .ok()
                .and_then(|value| Timestamp::from_second(value).ok()),
        ))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
        Ok(BackupDate(seconds_since_epoch(value)))
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(BackupDate(value.parse().ok()))
    }
}

/// Goes through [`std::time::Duration`] rather than casting.
fn seconds_since_epoch(seconds: f64) -> Option<Timestamp> {
    let elapsed = std::time::Duration::try_from_secs_f64(seconds).ok()?;
    let elapsed = SignedDuration::try_from(elapsed).ok()?;
    Timestamp::UNIX_EPOCH.checked_add(elapsed).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plist Aidoku actually writes, built from the same shape its encoder
    /// produces, so the tests exercise the real container rather than a JSON
    /// stand-in the app would never emit.
    fn plist_bytes(value: &plist::Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, value).expect("the fixture should encode");
        bytes
    }

    fn dict(pairs: Vec<(&str, plist::Value)>) -> plist::Value {
        plist::Value::Dictionary(
            pairs
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    fn array(values: Vec<plist::Value>) -> plist::Value {
        plist::Value::Array(values)
    }

    fn string(value: &str) -> plist::Value {
        plist::Value::String(value.to_string())
    }

    fn date(millis: i64) -> plist::Value {
        let timestamp = Timestamp::from_millisecond(millis).unwrap();
        plist::Value::Date(std::time::SystemTime::from(timestamp).into())
    }

    fn manga(source_id: &str, id: &str) -> plist::Value {
        dict(vec![
            ("id", string(id)),
            ("sourceId", string(source_id)),
            ("title", string(id)),
            ("status", plist::Value::Integer(0.into())),
            ("nsfw", plist::Value::Integer(0.into())),
            ("viewer", plist::Value::Integer(0.into())),
        ])
    }

    fn chapter_value(source_id: &str, manga_id: &str, id: &str, order: i64) -> plist::Value {
        dict(vec![
            ("id", string(id)),
            ("sourceId", string(source_id)),
            ("mangaId", string(manga_id)),
            ("title", string(id)),
            ("lang", string("en")),
            ("sourceOrder", plist::Value::Integer(order.into())),
        ])
    }

    fn history_value(source_id: &str, manga_id: &str, chapter_id: &str) -> plist::Value {
        dict(vec![
            ("sourceId", string(source_id)),
            ("mangaId", string(manga_id)),
            ("chapterId", string(chapter_id)),
            ("dateRead", date(1_700_000_000_000)),
            ("progress", plist::Value::Integer(4.into())),
            ("completed", plist::Value::Boolean(true)),
        ])
    }

    fn library_value(source_id: &str, manga_id: &str, categories: Vec<&str>) -> plist::Value {
        dict(vec![
            ("sourceId", string(source_id)),
            ("mangaId", string(manga_id)),
            ("dateAdded", date(1_600_000_000_000)),
            ("lastOpened", date(1_600_000_000_000)),
            ("lastUpdated", date(1_600_000_000_000)),
            (
                "categories",
                array(categories.into_iter().map(string).collect()),
            ),
        ])
    }

    fn backup(pairs: Vec<(&str, plist::Value)>) -> plist::Value {
        let mut pairs = pairs;
        pairs.push(("date", date(1_700_000_000_000)));
        dict(pairs)
    }

    fn read(value: &plist::Value) -> (Backup, ReadReport) {
        AidokuReader::new()
            .read_backup(&plist_bytes(value))
            .expect("a backup we just encoded should read back")
    }

    #[test]
    fn sources_become_extensions_keyed_by_id() {
        let (backup, report) = read(&backup(vec![
            ("version", string("0.8.4")),
            ("sources", array(vec![string("en.asurascans")])),
            ("manga", array(vec![manga("en.asurascans", "/series/1")])),
        ]));

        assert_eq!(backup.reader, Aidoku::ID);
        assert_eq!(backup.version.as_deref(), Some("0.8.4"));
        assert_eq!(backup.extensions.len(), 1);
        assert_eq!(backup.extensions[0].external_id, "en.asurascans");
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn a_source_id_carries_the_only_name_and_language_a_backup_has() {
        let (backup, _) = read(&backup(vec![(
            "sources",
            array(vec![
                string("en.asurascans"),
                string("multi.mangadex"),
                string("komga"),
            ]),
        )]));

        let described: Vec<(&str, &Language)> = backup
            .extensions
            .iter()
            .map(|extension| (extension.name.as_str(), &extension.lang))
            .collect();

        assert_eq!(
            described,
            vec![
                ("asurascans", &Language::from_code("en").unwrap()),
                // `multi` is Aidoku's spelling of "every language".
                ("mangadex", &Language::All),
                // Not `<language>.<site>` at all, so the id is the whole name
                // and the language is absent rather than claimed.
                ("komga", &Language::Unknown),
            ]
        );
    }

    #[test]
    fn entries_share_one_arc_with_the_extension_list() {
        // The invariant `ExtensionMapping` depends on: equal-looking `Extension`s hash the same
        // but resolve to nothing, so this has to be pointer identity, not equality.
        let (backup, _) = read(&backup(vec![
            ("sources", array(vec![string("en.asurascans")])),
            (
                "manga",
                array(vec![
                    manga("en.asurascans", "/a"),
                    manga("en.asurascans", "/b"),
                ]),
            ),
        ]));

        assert_eq!(backup.entries.len(), 2);
        for entry in &backup.entries {
            assert!(Arc::ptr_eq(&entry.extension, &backup.extensions[0]));
        }
    }

    #[test]
    fn an_uninstalled_source_still_shares_an_arc() {
        // `sources` lists what is installed *now*, and a library outlives an
        // uninstall. Synthesising a per-entry `Extension` here is exactly the
        // reader bug `PruneReport::unknown` exists to catch.
        let (backup, _) = read(&backup(vec![(
            "manga",
            array(vec![manga("en.mangago", "/a"), manga("en.mangago", "/b")]),
        )]));

        assert_eq!(backup.extensions.len(), 1);
        assert_eq!(backup.extensions[0].external_id, "en.mangago");
        for entry in &backup.entries {
            assert!(Arc::ptr_eq(&entry.extension, &backup.extensions[0]));
        }
    }

    #[test]
    fn local_entries_are_skipped_and_reported_once() {
        let (backup, report) = read(&backup(vec![
            (
                "sources",
                array(vec![string(LOCAL_SOURCE_ID), string("en.asurascans")]),
            ),
            (
                "manga",
                array(vec![
                    manga(LOCAL_SOURCE_ID, "/local-a"),
                    manga(LOCAL_SOURCE_ID, "/local-b"),
                    manga("en.asurascans", "/real"),
                ]),
            ),
        ]));

        assert_eq!(backup.entries.len(), 1);
        assert_eq!(backup.entries[0].id, "/real");
        // Two local entries, one report.
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].extension.external_id, LOCAL_SOURCE_ID);
        // It does not belong in the mapping: there is nothing to install for it.
        assert_eq!(backup.extensions.len(), 1);
        assert_eq!(backup.extensions[0].external_id, "en.asurascans");
    }

    #[test]
    fn a_history_row_is_what_makes_a_chapter_read() {
        let (backup, _) = read(&backup(vec![
            ("manga", array(vec![manga("en.asurascans", "/series")])),
            (
                "chapters",
                array(vec![
                    chapter_value("en.asurascans", "/series", "/series/c1", 0),
                    chapter_value("en.asurascans", "/series", "/series/c2", 1),
                ]),
            ),
            (
                "history",
                array(vec![history_value(
                    "en.asurascans",
                    "/series",
                    "/series/c1",
                )]),
            ),
        ]));
        let entry = &backup.entries[0];

        assert!(entry.chapters[0].read);
        assert_eq!(entry.chapters[0].last_page_read, 4);
        assert_eq!(
            entry.chapters[0].last_read,
            Some(Timestamp::from_millisecond(1_700_000_000_000).unwrap())
        );
        // A chapter with no history row is simply unread, not absent.
        assert!(!entry.chapters[1].read);
        assert_eq!(entry.chapters[1].last_page_read, 0);
        // The IR reserves `Manga::history` for chapters that are *not* in the list.
        assert!(entry.history.is_empty());
    }

    #[test]
    fn history_for_a_missing_chapter_falls_back_to_the_history_list() {
        let (backup, _) = read(&backup(vec![
            ("manga", array(vec![manga("en.asurascans", "/series")])),
            (
                "chapters",
                array(vec![chapter_value(
                    "en.asurascans",
                    "/series",
                    "/series/c1",
                    0,
                )]),
            ),
            (
                "history",
                array(vec![history_value(
                    "en.asurascans",
                    "/series",
                    "/series/gone",
                )]),
            ),
        ]));
        let entry = &backup.entries[0];

        assert!(entry.chapters[0].last_read.is_none());
        assert_eq!(entry.history.len(), 1);
        assert_eq!(entry.history[0].chapter_id.as_deref(), Some("/series/gone"));
        // Aidoku's history rows have no URL on them at all.
        assert_eq!(entry.history[0].url, None);
    }

    #[test]
    fn history_and_chapters_only_join_within_one_entry() {
        // Every id in a backup is scoped to its source, so two sources numbering
        // a chapter the same way must not bleed into each other.
        let (backup, _) = read(&backup(vec![
            (
                "manga",
                array(vec![manga("en.a", "/series"), manga("en.b", "/series")]),
            ),
            (
                "chapters",
                array(vec![
                    chapter_value("en.a", "/series", "/c1", 0),
                    chapter_value("en.b", "/series", "/c1", 0),
                ]),
            ),
            (
                "history",
                array(vec![history_value("en.a", "/series", "/c1")]),
            ),
        ]));

        assert!(backup.entries[0].chapters[0].read);
        assert!(!backup.entries[1].chapters[0].read);
    }

    #[test]
    fn a_repeated_row_reads_the_way_aidoku_restores_it() {
        // Real backups repeat both kinds of row. `BackupManager` uniques its
        // history lookup `{ first, _ in first }` and then hands *every* chapter
        // row the matching history object, so a duplicated chapter is read
        // twice over rather than once.
        let mut later = history_value("en.asurascans", "/series", "/c1");
        if let plist::Value::Dictionary(fields) = &mut later {
            fields.insert("progress".to_string(), plist::Value::Integer(99.into()));
        }

        let (backup, _) = read(&backup(vec![
            ("manga", array(vec![manga("en.asurascans", "/series")])),
            (
                "chapters",
                array(vec![
                    chapter_value("en.asurascans", "/series", "/c1", 0),
                    chapter_value("en.asurascans", "/series", "/c1", 1),
                ]),
            ),
            (
                "history",
                array(vec![
                    history_value("en.asurascans", "/series", "/c1"),
                    later,
                ]),
            ),
        ]));
        let chapters = &backup.entries[0].chapters;

        assert_eq!(chapters.len(), 2);
        assert!(chapters.iter().all(|chapter| chapter.read));
        // The first row won, so neither copy picked up the later progress.
        assert_eq!(chapters[0].last_page_read, 4);
        assert_eq!(chapters[1].last_page_read, 4);
        // And a chapter that claimed a row is not also an orphan.
        assert!(backup.entries[0].history.is_empty());
    }

    #[test]
    fn a_negative_page_is_a_chapter_no_page_was_recorded_for() {
        let mut row = history_value("en.asurascans", "/series", "/c1");
        if let plist::Value::Dictionary(fields) = &mut row {
            fields.insert("progress".to_string(), plist::Value::Integer((-1).into()));
        }

        let (backup, _) = read(&backup(vec![
            ("manga", array(vec![manga("en.asurascans", "/series")])),
            (
                "chapters",
                array(vec![chapter_value("en.asurascans", "/series", "/c1", 0)]),
            ),
            ("history", array(vec![row])),
        ]));

        assert!(backup.entries[0].chapters[0].read);
        assert_eq!(backup.entries[0].chapters[0].last_page_read, 0);
    }

    #[test]
    fn chapters_come_back_in_the_order_the_source_listed_them() {
        let (backup, _) = read(&backup(vec![
            ("manga", array(vec![manga("en.asurascans", "/series")])),
            (
                "chapters",
                array(vec![
                    chapter_value("en.asurascans", "/series", "/c3", 2),
                    chapter_value("en.asurascans", "/series", "/c1", 0),
                    chapter_value("en.asurascans", "/series", "/c2", 1),
                ]),
            ),
        ]));

        let ids: Vec<&str> = backup.entries[0]
            .chapters
            .iter()
            .map(|chapter| chapter.id.as_str())
            .collect();
        assert_eq!(ids, ["/c1", "/c2", "/c3"]);
    }

    #[test]
    fn only_a_library_entry_is_a_favorite() {
        let (backup, _) = read(&backup(vec![
            (
                "manga",
                array(vec![
                    manga("en.asurascans", "/kept"),
                    manga("en.asurascans", "/read-once"),
                ]),
            ),
            (
                "library",
                array(vec![library_value(
                    "en.asurascans",
                    "/kept",
                    vec!["Currently Reading"],
                )]),
            ),
        ]));

        let kept = &backup.entries[0];
        assert!(kept.favorite);
        assert_eq!(
            kept.date_added,
            Some(Timestamp::from_millisecond(1_600_000_000_000).unwrap())
        );
        assert_eq!(
            kept.categories,
            vec![CategoryID("Currently Reading".to_string())]
        );

        // `manga` holds every series the database knows, not just the library.
        let read_once = &backup.entries[1];
        assert!(!read_once.favorite);
        assert_eq!(read_once.date_added, None);
        assert!(read_once.categories.is_empty());
    }

    #[test]
    fn categories_are_keyed_by_their_exact_title() {
        // `BackupManager` restores through `categoryByTitle[$0]`, so a stray
        // space is part of the key, and the `sort` index is not the key at all.
        let (backup, _) = read(&backup(vec![
            (
                "categories",
                array(vec![
                    dict(vec![
                        ("title", string("Comedy ")),
                        ("sort", plist::Value::Integer(4.into())),
                        ("group", plist::Value::Boolean(false)),
                    ]),
                    // An older backup spells a category as a bare title.
                    string("Isekai"),
                    // ...and one with no title is a row nothing can reference.
                    dict(vec![("sort", plist::Value::Integer(9.into()))]),
                ]),
            ),
            ("manga", array(vec![manga("en.asurascans", "/series")])),
            (
                "library",
                array(vec![library_value(
                    "en.asurascans",
                    "/series",
                    vec!["Comedy "],
                )]),
            ),
        ]));

        let ids: Vec<&str> = backup
            .categories
            .iter()
            .map(|category| category.id.0.as_str())
            .collect();
        assert_eq!(ids, ["Comedy ", "Isekai"]);
        assert_eq!(backup.categories[0].name, "Comedy ");
        assert_eq!(
            backup.entries[0].categories,
            vec![CategoryID("Comedy ".to_string())]
        );
    }

    #[test]
    fn statuses_follow_the_publishing_status_constants() {
        for (raw, expected) in [
            (0, MangaStatus::Unknown),
            (1, MangaStatus::Ongoing),
            (2, MangaStatus::Completed),
            (3, MangaStatus::Cancelled),
            (4, MangaStatus::OnHiatus),
            // `notPublished`, which the IR has no name for.
            (5, MangaStatus::Unknown),
            (99, MangaStatus::Unknown),
        ] {
            assert_eq!(manga_status(raw), expected, "status {raw}");
        }
    }

    #[test]
    fn only_the_nsfw_rating_reads_as_nsfw() {
        let rated = |rating: i64| {
            let mut entry = manga("en.asurascans", "/series");
            if let plist::Value::Dictionary(fields) = &mut entry {
                fields.insert("nsfw".to_string(), plist::Value::Integer(rating.into()));
            }
            let (backup, _) = read(&backup(vec![("manga", array(vec![entry]))]));
            backup.entries[0].nsfw
        };

        // `MangaContentRating`: safe, suggestive, nsfw.
        assert!(!rated(0));
        assert!(!rated(1));
        assert!(rated(2));
    }

    #[test]
    fn absent_and_empty_fields_both_read_as_none() {
        let mut entry = manga("en.asurascans", "/series");
        if let plist::Value::Dictionary(fields) = &mut entry {
            fields.insert("author".to_string(), string(""));
            fields.insert("cover".to_string(), string(""));
        }

        let (backup, _) = read(&backup(vec![("manga", array(vec![entry]))]));
        let entry = &backup.entries[0];

        assert_eq!(entry.author, None);
        // Never written at all.
        assert_eq!(entry.artist, None);
        assert_eq!(entry.cover_url, None);
        assert!(entry.tags.is_empty());
    }

    #[test]
    fn a_side_loaded_source_is_an_object_rather_than_an_id() {
        let (backup, _) = read(&backup(vec![(
            "sources",
            array(vec![dict(vec![
                ("id", string("en.custom")),
                ("apiVersion", string("0.7")),
                ("config", plist::Value::Data(vec![1, 2, 3])),
            ])]),
        )]));

        assert_eq!(backup.extensions.len(), 1);
        assert_eq!(backup.extensions[0].external_id, "en.custom");
    }

    #[test]
    fn a_json_backup_reads_the_same_as_a_plist_one() {
        // `Backup.load` falls back to JSON with `.secondsSince1970`, so a backup
        // old enough to predate the plist still has to read here.
        let value = backup(vec![
            ("sources", array(vec![string("en.asurascans")])),
            ("manga", array(vec![manga("en.asurascans", "/series")])),
            (
                "library",
                array(vec![library_value("en.asurascans", "/series", vec![])]),
            ),
        ]);
        let (from_plist, _) = read(&value);

        let json = r#"{
            "date": 1700000000,
            "sources": ["en.asurascans"],
            "manga": [{ "id": "/series", "sourceId": "en.asurascans",
                        "title": "/series", "status": 0, "nsfw": 0, "viewer": 0 }],
            "library": [{ "sourceId": "en.asurascans", "mangaId": "/series",
                          "dateAdded": 1600000000, "lastOpened": 1600000000,
                          "lastUpdated": 1600000000, "categories": [] }]
        }"#;
        let (from_json, _) = AidokuReader::new().read_backup(json.as_bytes()).unwrap();

        assert_eq!(from_plist, from_json);
    }

    #[test]
    fn a_page_index_too_large_for_the_ir_clamps_instead_of_wrapping() {
        assert_eq!(page_index(i64::from(i32::MAX) + 1), i32::MAX);
        assert_eq!(page_index(-5), 0);
    }

    #[test]
    fn bytes_that_are_not_a_backup_are_rejected() {
        let error = AidokuReader::new()
            .read_backup(&[0xff, 0xff, 0xff, 0xff])
            .unwrap_err();

        assert!(matches!(error, ReadError::Malformed(_)), "{error:?}");
    }

    /// Reads a real backup end to end. Ignored because it needs a file this repo does not ship;
    /// point `AIB` at one, or drop a `backup.aib` in the workspace root.
    ///
    /// ```sh
    /// cargo test -p converter-aidoku -- --ignored real_backup --nocapture
    /// ```
    #[ignore = "needs a real backup on disk"]
    #[test]
    // The counts are the point of running it by hand; `--nocapture` is how you see them.
    #[allow(clippy::print_stdout)]
    fn real_backup_round_trips() {
        let path = std::env::var("AIB").unwrap_or_else(|_| "../../backup.aib".to_string());
        let bytes = std::fs::read(&path).expect("set AIB to an .aib");
        let (backup, report) = AidokuReader::new().read_backup(&bytes).unwrap();

        let favorites = backup.entries.iter().filter(|entry| entry.favorite).count();
        let chapters: usize = backup
            .entries
            .iter()
            .map(|entry| entry.chapters.len())
            .sum();
        println!(
            "{} entries ({favorites} in the library), {chapters} chapters, {} extensions, \
             {} categories, {} sources skipped",
            backup.entries.len(),
            backup.extensions.len(),
            backup.categories.len(),
            report.skipped.len(),
        );

        // Every entry points at a *listed* extension by pointer, not merely an equal one. This is
        // the invariant `ExtensionMapping` relies on, and the one a real library actually stresses.
        let unlisted = backup
            .entries
            .iter()
            .filter(|entry| {
                !backup
                    .extensions
                    .iter()
                    .any(|listed| Arc::ptr_eq(listed, &entry.extension))
            })
            .count();
        assert_eq!(unlisted, 0, "entries carrying an extension nobody listed");

        // Two sources collapsing onto one id would silently merge libraries.
        let ids: HashSet<&str> = backup
            .extensions
            .iter()
            .map(|extension| extension.external_id.as_str())
            .collect();
        assert_eq!(ids.len(), backup.extensions.len(), "duplicate external_id");

        // A category is referenced by its exact title, trailing spaces and all,
        // so a reader that trimmed or renumbered them leaves these dangling.
        // Assert the check is not vacuous first.
        let known: HashSet<&str> = backup
            .categories
            .iter()
            .map(|category| category.id.0.as_str())
            .collect();
        let refs: Vec<&CategoryID> = backup
            .entries
            .iter()
            .flat_map(|entry| &entry.categories)
            .collect();
        assert!(!refs.is_empty(), "no category references to test against");
        let dangling = refs
            .iter()
            .filter(|id| !known.contains(id.0.as_str()))
            .count();
        assert_eq!(
            dangling,
            0,
            "{dangling} of {} category refs dangle",
            refs.len()
        );

        // Read state lives entirely in the history join; losing it would leave
        // a library that looks untouched.
        let read = backup
            .entries
            .iter()
            .flat_map(|entry| &entry.chapters)
            .filter(|chapter| chapter.read)
            .count();
        assert!(read > 0, "no chapter came back read");
    }

    #[test]
    fn an_out_of_range_plist_date_does_not_panic() {
        // Write a safe date through plist, find the `0x33` tag in
        // the binary, replace the 8 IEEE-754 bytes after it with
        // `f64::MAX`, and parse back. The bomb duration is far past
        // year 9999, so `Date::to_xml_format` panics without
        // `catch_unwind`.
        let date: plist::Date = std::time::SystemTime::UNIX_EPOCH.into();
        let dict = plist::Value::Dictionary(
            vec![("d".to_string(), plist::Value::Date(date))]
                .into_iter()
                .collect(),
        );
        let mut bytes = Vec::new();
        plist::to_writer_binary(&mut bytes, &dict).unwrap();

        // The only date tag byte in this one-key dict is our date.
        let pos = bytes
            .iter()
            .position(|&b| b == 0x33)
            .expect("a plist with a Date must contain 0x33");
        bytes[pos + 1..pos + 9].copy_from_slice(&1e15_f64.to_be_bytes());

        let result = AidokuReader::new().read_backup(&bytes);
        assert!(
            matches!(result, Err(ReadError::Malformed(_))),
            "expected a malformed error, got {result:?}"
        );
    }

    #[test]
    fn a_negative_float_date_from_json_degrades_to_none() {
        // `Backup.load` uses `.secondsSince1970`, so old JSON
        // backups spell dates as plain floats. A negative one
        // cannot round-trip through Duration and used to reject
        // the entire file. Now it degrades to None.
        let json = r#"{
            "date": 1700000000,
            "sources": ["en.asurascans"],
            "manga": [{ "id": "/series", "sourceId": "en.asurascans",
                        "title": "/series", "status": 0, "nsfw": 0, "viewer": 0 }],
            "chapters": [{ "id": "/c1", "sourceId": "en.asurascans",
                           "mangaId": "/series", "title": "/c1",
                           "dateUploaded": -86400.5 }]
        }"#;

        let (backup, _) = AidokuReader::new()
            .read_backup(json.as_bytes())
            .expect("a negative float date should not take down the whole file");

        assert_eq!(backup.entries[0].chapters[0].date_uploaded, None);
    }
}
