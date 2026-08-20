use std::collections::{HashMap, HashSet};

use converter_core::extensions::lang::Language;
use converter_core::extensions::types::Extension;
use converter_core::io::writer::{BackupWriter, WriteReport};
use converter_core::io::{FailedExtension, SkippedExtension, WriteError};
use converter_core::ir::types::{Backup, Category, CategoryID, Chapter, Manga, MangaStatus};
use converter_core::orchestrator::mapping::ExtensionMapping;
use jiff::Timestamp;
use serde::Serialize;

use crate::Aidoku;

/// `MangaObject.nsfw` is a rating rather than a flag; 2 is the one the reader
/// reads back as nsfw.
const NSFW_RATING: i64 = 2;
/// `MangaObject.viewer` is required and means "whatever the source says", which
/// is the only honest answer for an entry that came from another app.
const DEFAULT_VIEWER: i64 = 0;

/// What a chapter, a history row and a library row all have to agree on: Aidoku
/// scopes every id to the source that produced it, and restores by the pair.
type MangaKey = (String, String);

pub struct AidokuWriter {}

impl AidokuWriter {
    pub const fn new() -> Self {
        Self {}
    }
}

impl Default for AidokuWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl BackupWriter for AidokuWriter {
    converter_core::format_id!(Aidoku);

    fn write_backup(
        &self,
        backup: &Backup,
        mapping: &ExtensionMapping,
    ) -> Result<(Vec<u8>, WriteReport), WriteError> {
        let (raw, report) = assemble(backup, mapping, Timestamp::now());

        let mut bytes = Vec::new();
        // Aidoku writes a binary property list and reads that before it falls
        // back to JSON, so the format it writes itself is the one to hand back.
        plist::to_writer_binary(&mut bytes, &raw)
            .map_err(|error| WriteError::Encode(error.to_string()))?;

        Ok((bytes, report))
    }
}

/// `now` stands in for the dates the IR doesn't carry, so a caller that wants a
/// reproducible file can pin it.
fn assemble(
    backup: &Backup,
    mapping: &ExtensionMapping,
    now: Timestamp,
) -> (RawBackup, WriteReport) {
    let mut report = WriteReport::default();
    let (targets, sources) = targets(backup, mapping, &mut report);

    let (categories, titles) = categories(&backup.categories);

    let mut contents = Contents::default();
    let mut unknown: HashSet<&Extension> = HashSet::new();
    let mut written: HashSet<MangaKey> = HashSet::new();

    for entry in &backup.entries {
        let Some(target) = targets.get(entry.extension.as_ref()) else {
            // `Orchestrator::convert` catches this before the writer runs, but
            // `write` is callable on its own, and a backup silently short a few
            // entries hides the reader bug behind it.
            if unknown.insert(entry.extension.as_ref()) {
                report.failed.push(FailedExtension {
                    reason: "not in the backup's extension table, so nothing resolved it; \
                             the reader must share one `Arc<Extension>` between \
                             `Backup::extensions` and the entries that use it"
                        .to_string(),
                    extension: entry.extension.as_ref().clone(),
                });
            }
            continue;
        };
        // Already reported by `targets`; the entries behind it just go.
        let Some(target) = target else { continue };

        let key = (target.external_id.clone(), entry.id.clone());
        // Two of the source app's extensions can map onto one Aidoku source,
        // and Aidoku restores by `(sourceId, mangaId)`: writing both copies
        // would put the same series in the library twice.
        if !written.insert(key.clone()) {
            continue;
        }

        contents.push_entry(entry, target, &key, &titles, now);
    }

    let Contents {
        library,
        history,
        manga,
        chapters,
    } = contents;

    let backup = RawBackup {
        library,
        history,
        manga,
        chapters,
        categories,
        sources,
        date: date_or_epoch(now),
        // Keeps this out of the pool `cleanUpAutoBackups` prunes: an imported
        // backup the app deletes on its own schedule is a nasty surprise.
        automatic: false,
        // Nothing reads this back but the backup list, and the version the
        // entries actually came from is more use there than an Aidoku release
        // this file was never written by.
        version: backup.version.clone(),
    };

    (backup, report)
}

/// The Aidoku source behind each of the backup's extensions, or `None` for the
/// ones nothing can be written under. Both cases are reported here, so the
/// entry loop can drop their entries without repeating itself.
fn targets<'a>(
    backup: &'a Backup,
    mapping: &'a ExtensionMapping,
    report: &mut WriteReport,
) -> (HashMap<&'a Extension, Option<&'a Extension>>, Vec<String>) {
    let mut targets = HashMap::with_capacity(backup.extensions.len());
    let mut sources: Vec<String> = Vec::new();

    for extension in &backup.extensions {
        let target = mapping.get(extension).map(AsRef::as_ref);

        let usable = match target {
            None => {
                report.skipped.push(SkippedExtension {
                    reason: "no Aidoku source to point it at".to_string(),
                    extension: extension.as_ref().clone(),
                });
                None
            }
            // Aidoku keys everything it restores off the source id, so a target
            // without one is a row the app would drop on the floor anyway.
            Some(target) if target.external_id.is_empty() => {
                report.failed.push(FailedExtension {
                    reason: format!(
                        "matched Aidoku source '{}', which carries no source id",
                        target.name
                    ),
                    extension: extension.as_ref().clone(),
                });
                None
            }
            Some(target) => Some(target),
        };

        // Several of the backup's extensions can share one Aidoku source.
        if let Some(target) = usable
            && !sources.contains(&target.external_id)
        {
            sources.push(target.external_id.clone());
        }
        targets.insert(extension.as_ref(), usable);
    }

    (targets, sources)
}

/// Aidoku's library rows name their categories by title, so the IR's ids have
/// to be spent here. Two categories sharing a title would collide on restore
/// (`categoryByTitle` keeps the first), so the duplicate's id is pointed at the
/// one that survives instead of being written twice.
fn categories(list: &[Category]) -> (Vec<RawCategory>, HashMap<&CategoryID, &str>) {
    let mut categories = Vec::with_capacity(list.len());
    let mut titles: HashMap<&CategoryID, &str> = HashMap::with_capacity(list.len());
    let mut seen: HashMap<&str, &str> = HashMap::new();

    for category in list {
        let title = category.name.as_str();
        // A row with no title is one no library entry can reference.
        if title.is_empty() {
            continue;
        }

        if let Some(existing) = seen.get(title) {
            titles.insert(&category.id, existing);
            continue;
        }

        categories.push(RawCategory {
            title: title.to_string(),
            sort: narrow(categories.len()),
            group: false,
        });
        seen.insert(title, title);
        titles.insert(&category.id, title);
    }

    (categories, titles)
}

#[derive(Default)]
struct Contents {
    library: Vec<RawLibraryManga>,
    history: Vec<RawHistory>,
    manga: Vec<RawManga>,
    chapters: Vec<RawChapter>,
}

impl Contents {
    fn push_entry(
        &mut self,
        entry: &Manga,
        target: &Extension,
        key: &MangaKey,
        titles: &HashMap<&CategoryID, &str>,
        now: Timestamp,
    ) {
        let (source_id, manga_id) = key;

        self.manga.push(RawManga {
            id: manga_id.clone(),
            source_id: source_id.clone(),
            title: entry.title.clone(),
            author: entry.author.clone(),
            artist: entry.artist.clone(),
            desc: entry.description.clone(),
            tags: entry.tags.clone(),
            cover: entry.cover_url.clone(),
            status: manga_status(&entry.status),
            nsfw: if entry.nsfw { NSFW_RATING } else { 0 },
            viewer: DEFAULT_VIEWER,
        });

        let lang = chapter_lang(&entry.extension.lang, &target.lang);
        let mut ids: HashSet<&str> = HashSet::with_capacity(entry.chapters.len());
        let mut order = 0;

        for chapter in &entry.chapters {
            // The same chapter twice would be two rows in the reader, and the
            // second would take the first's place in the source order.
            if !ids.insert(chapter.id.as_str()) {
                continue;
            }

            self.chapters.push(RawChapter {
                source_id: source_id.clone(),
                manga_id: manga_id.clone(),
                id: chapter.id.clone(),
                title: non_empty(&chapter.title),
                scanlator: chapter.scanlator.clone(),
                lang: lang.clone(),
                chapter: chapter.chapter_number,
                date_uploaded: chapter.date_uploaded.and_then(date),
                source_order: narrow(order),
            });
            order += 1;

            if let Some(row) = history_row(chapter, key) {
                self.history.push(row);
            }
        }

        for row in &entry.history {
            // A reader that keys history by URL keys its chapters the same way,
            // so the URL is the chapter id for the rows whose chapter is gone.
            let Some(id) = row.chapter_id.as_deref().or(row.url.as_deref()) else {
                continue;
            };
            if !ids.insert(id) {
                continue;
            }

            self.history.push(RawHistory {
                date_read: date_or_epoch(row.last_read),
                source_id: source_id.clone(),
                manga_id: manga_id.clone(),
                chapter_id: id.to_string(),
                progress: 0,
                completed: true,
            });
        }

        if entry.favorite {
            self.library.push(library_row(entry, key, titles, now));
        }
    }
}

fn library_row(
    entry: &Manga,
    key: &MangaKey,
    titles: &HashMap<&CategoryID, &str>,
    now: Timestamp,
) -> RawLibraryManga {
    let (source_id, manga_id) = key;
    let added = entry.date_added.unwrap_or(now);
    let last_read = last_read(entry);

    let mut categories: Vec<String> = Vec::new();
    for id in &entry.categories {
        // A category the backup never listed is one the restore would drop.
        if let Some(title) = titles.get(id)
            && !categories.iter().any(|kept| kept == title)
        {
            categories.push((*title).to_string());
        }
    }

    RawLibraryManga {
        last_opened: date_or_epoch(last_read.unwrap_or(added)),
        last_updated: date_or_epoch(added),
        last_read: last_read.and_then(date),
        date_added: date_or_epoch(added),
        categories,
        manga_id: manga_id.clone(),
        source_id: source_id.clone(),
    }
}

fn last_read(entry: &Manga) -> Option<Timestamp> {
    entry
        .chapters
        .iter()
        .filter_map(|chapter| chapter.last_read)
        .chain(entry.history.iter().map(|row| row.last_read))
        .max()
}

/// Aidoku keeps no read flag on a chapter: the reader marks one read by the
/// history row that points at it, so a chapter with anything to say about its
/// progress needs a row of its own.
fn history_row(chapter: &Chapter, key: &MangaKey) -> Option<RawHistory> {
    if !chapter.read && chapter.last_read.is_none() && chapter.last_page_read <= 0 {
        return None;
    }

    let (source_id, manga_id) = key;

    Some(RawHistory {
        // `dateRead` is not optional in the app's decoder, and a chapter read at
        // an unknown time is better placed at the start of the history than at
        // the top of it.
        date_read: date_or_epoch(chapter.last_read.unwrap_or(Timestamp::UNIX_EPOCH)),
        source_id: source_id.clone(),
        manga_id: manga_id.clone(),
        chapter_id: chapter.id.clone(),
        progress: narrow_i32(chapter.last_page_read),
        completed: chapter.read,
    })
}

/// Chapters carry their own language in Aidoku, which the IR has no room for.
/// The source the entry came from is the closer answer of the two: a
/// single-language extension really did serve single-language chapters, even
/// where the Aidoku source it maps onto covers everything.
fn chapter_lang(source: &Language, target: &Language) -> String {
    let lang = match source {
        Language::All | Language::Unknown => target,
        specific @ Language::Tagged { .. } => specific,
    };
    match lang {
        Language::All | Language::Unknown => String::new(),
        specific @ Language::Tagged { .. } => specific.to_code(),
    }
}

/// Mirrors `PublishingStatus`, which the database stores raw. Neither of the
/// two the IR has left over is a status Aidoku knows: `notPublished` is the
/// only value it has that the IR doesn't, and it is not what either means.
const fn manga_status(status: &MangaStatus) -> i64 {
    match status {
        MangaStatus::Ongoing => 1,
        // A finished serialisation is what Aidoku calls completed; it draws no
        // line between that and a finished publication.
        MangaStatus::Completed | MangaStatus::PublishingFinished => 2,
        MangaStatus::Cancelled => 3,
        MangaStatus::OnHiatus => 4,
        MangaStatus::Unknown | MangaStatus::Licensed => 0,
    }
}

/// Every number here lands in an `Int16` Core Data column, and Swift's
/// `Int16(_:)` traps rather than wraps, so a value that doesn't fit would take
/// the app down in the middle of a restore.
fn narrow(value: usize) -> i64 {
    i16::try_from(value).map_or_else(|_| i64::from(i16::MAX), i64::from)
}

fn narrow_i32(value: i32) -> i64 {
    i64::from(value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)))
}

fn date(timestamp: Timestamp) -> Option<plist::Date> {
    // `plist::Date`'s `Serialize` impl panics on a date whose year is before
    // 0000, which RFC 3339 cannot render. A timestamp that old would come from
    // a JSON-fallback backup that spelled a pre-epoch date as `dateUploaded`.
    if timestamp.as_second() < -62_167_219_200 {
        return None;
    }
    Some(std::time::SystemTime::from(timestamp).into())
}

fn date_or_epoch(timestamp: Timestamp) -> plist::Date {
    date(timestamp).unwrap_or_else(|| std::time::SystemTime::from(Timestamp::UNIX_EPOCH).into())
}

/// The IR means "absent" by `None`; a chapter with no title has an empty one.
fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// Every list is left out when it is empty rather than written as one: a
/// restore clears the table behind each key the backup carries, so an empty
/// `trackItems` or `categories` would wipe what the user already had. The keys
/// the IR knows nothing about - trackers, reading sessions, updates, settings -
/// are missing here for the same reason. `sourceLists` is the loudest of them:
/// restoring one replaces every list the user has subscribed to.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RawBackup {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    library: Vec<RawLibraryManga>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    history: Vec<RawHistory>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    manga: Vec<RawManga>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    chapters: Vec<RawChapter>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    categories: Vec<RawCategory>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sources: Vec<String>,
    /// The one key the app insists on: a backup without it doesn't list.
    date: plist::Date,
    automatic: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RawManga {
    id: String,
    source_id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    desc: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cover: Option<String>,
    status: i64,
    nsfw: i64,
    viewer: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RawChapter {
    source_id: String,
    manga_id: String,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scanlator: Option<String>,
    lang: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    chapter: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date_uploaded: Option<plist::Date>,
    source_order: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RawHistory {
    date_read: plist::Date,
    source_id: String,
    manga_id: String,
    chapter_id: String,
    progress: i64,
    completed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RawLibraryManga {
    last_opened: plist::Date,
    last_updated: plist::Date,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_read: Option<plist::Date>,
    date_added: plist::Date,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    categories: Vec<String>,
    manga_id: String,
    source_id: String,
}

#[derive(Debug, Serialize)]
struct RawCategory {
    title: String,
    sort: i64,
    group: bool,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use converter_core::extensions::types::ExtensionRepositoryInfo;
    use converter_core::io::reader::BackupReader;
    use converter_core::ir::types::History;

    use super::*;
    use crate::reader::AidokuReader;

    /// A source as the reader hands it back: a backup names its sources by id
    /// and records nothing else about them, so anything else here would fail a
    /// round trip for a reason the writer isn't responsible for.
    fn extension(id: &str) -> Arc<Extension> {
        let (name, lang) = id
            .split_once('.')
            .map_or((id, Language::All), |(lang, site)| {
                (site, Language::from_code(lang).unwrap())
            });

        Arc::new(Extension {
            reader: Aidoku::ID,
            name: name.to_string(),
            external_id: id.to_string(),
            url: String::new(),
            lang,
            repo: ExtensionRepositoryInfo::new(Aidoku::ID, None, None, None, None),
        })
    }

    fn timestamp(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap()
    }

    const NOW: i64 = 1_800_000_000;

    fn chapter(id: &str) -> Chapter {
        Chapter {
            id: id.to_string(),
            title: format!("Chapter {id}"),
            read: false,
            last_page_read: 0,
            scanlator: None,
            // Aidoku records neither, so neither survives a round trip.
            date_fetched: None,
            last_modified: None,
            date_uploaded: Some(timestamp(1_700_000_000)),
            chapter_number: Some(1.0),
            last_read: None,
        }
    }

    fn entry(id: &str, extension: &Arc<Extension>) -> Manga {
        Manga {
            id: id.to_string(),
            title: format!("Series {id}"),
            extension: Arc::clone(extension),
            status: MangaStatus::Ongoing,
            nsfw: false,
            favorite: true,
            author: None,
            artist: None,
            description: None,
            chapters: Vec::new(),
            history: Vec::new(),
            tags: Vec::new(),
            categories: Vec::new(),
            cover_url: None,
            date_added: Some(timestamp(1_600_000_000)),
        }
    }

    fn backup(extensions: Vec<Arc<Extension>>, entries: Vec<Manga>) -> Backup {
        Backup {
            reader: Aidoku::ID,
            version: Some("0.8.4".to_string()),
            extensions,
            entries,
            categories: Vec::new(),
        }
    }

    /// Every extension mapped onto itself, which is what an Aidoku backup
    /// converted to Aidoku resolves to.
    fn identity(backup: &Backup) -> ExtensionMapping {
        let mut mapping = ExtensionMapping::new(Aidoku::ID);
        for extension in &backup.extensions {
            mapping
                .insert(Arc::clone(extension), Arc::clone(extension))
                .unwrap();
        }
        mapping
    }

    fn written(backup: &Backup, mapping: &ExtensionMapping) -> (plist::Value, WriteReport) {
        let (bytes, report) = AidokuWriter::new()
            .write_backup(backup, mapping)
            .expect("the backup should encode");

        let value = plist::Value::from_reader(std::io::Cursor::new(bytes))
            .expect("the writer should produce a property list");

        (value, report)
    }

    fn dictionary(value: &plist::Value) -> &plist::Dictionary {
        value.as_dictionary().expect("a backup is a dictionary")
    }

    fn array<'a>(value: &'a plist::Value, key: &str) -> &'a [plist::Value] {
        dictionary(value)
            .get(key)
            .and_then(plist::Value::as_array)
            .unwrap_or_else(|| panic!("the backup should carry {key}"))
    }

    fn field<'a>(row: &'a plist::Value, key: &str) -> &'a plist::Value {
        dictionary(row)
            .get(key)
            .unwrap_or_else(|| panic!("the row should carry {key}"))
    }

    #[test]
    fn round_trips_through_the_reader() {
        let mangapill = extension("en.mangapill");
        let mut read = chapter("/chapters/1");
        read.read = true;
        read.last_read = Some(timestamp(1_700_000_100));
        read.scanlator = Some("Team".to_string());

        let mut in_progress = chapter("/chapters/2");
        in_progress.last_page_read = 4;
        in_progress.last_read = Some(timestamp(1_700_000_200));

        let mut series = entry("/manga/1", &mangapill);
        series.author = Some("Author".to_string());
        series.artist = Some("Artist".to_string());
        series.description = Some("A description".to_string());
        series.tags = vec!["Action".to_string(), "Drama".to_string()];
        series.cover_url = Some("https://example.test/1.jpg".to_string());
        series.nsfw = true;
        series.status = MangaStatus::Completed;
        series.categories = vec![CategoryID("Reading".to_string())];
        series.chapters = vec![read, in_progress, chapter("/chapters/3")];
        // A chapter the backup no longer holds, which is the only thing
        // `Manga::history` is for.
        series.history = vec![History {
            chapter_id: Some("/chapters/0".to_string()),
            url: None,
            last_read: timestamp(1_690_000_000),
        }];

        let mut unfavorited = entry("/manga/2", &mangapill);
        unfavorited.favorite = false;
        // Aidoku keeps the date a series was added on its library row, so an
        // entry outside the library has nowhere to put one.
        unfavorited.date_added = None;

        let mut original = backup(vec![mangapill], vec![series, unfavorited]);
        original.categories = vec![Category {
            id: CategoryID("Reading".to_string()),
            name: "Reading".to_string(),
            last_modified: Timestamp::UNIX_EPOCH,
        }];

        let (bytes, report) = AidokuWriter::new()
            .write_backup(&original, &identity(&original))
            .expect("the backup should encode");
        assert_eq!(report, WriteReport::default());

        let (round_tripped, _) = AidokuReader::new()
            .read_backup(&bytes)
            .expect("the reader should take what the writer wrote");

        assert_eq!(round_tripped, original);
    }

    #[test]
    fn a_chapter_read_at_an_unknown_time_lands_at_the_start_of_the_history() {
        let source = extension("en.mangapill");
        let mut chapter = chapter("/chapters/1");
        chapter.read = true;

        let mut entry = entry("/manga/1", &source);
        entry.chapters = vec![chapter];
        let backup = backup(vec![Arc::clone(&source)], vec![entry]);

        let (value, _) = written(&backup, &identity(&backup));
        let row = &array(&value, "history")[0];

        // `dateRead` is not optional in the app's decoder, so a chapter another
        // app called read without saying when still needs a date. The epoch
        // keeps it out of the way of everything the user really did read last.
        assert_eq!(
            field(row, "dateRead")
                .as_date()
                .map(std::time::SystemTime::from),
            Some(std::time::SystemTime::from(Timestamp::UNIX_EPOCH))
        );
        assert_eq!(field(row, "completed").as_boolean(), Some(true));
    }

    #[test]
    fn leaves_out_the_tables_it_has_nothing_to_say_about() {
        let extension = extension("en.mangapill");
        let backup = backup(
            vec![Arc::clone(&extension)],
            vec![entry("/manga/1", &extension)],
        );

        let (value, _) = written(&backup, &identity(&backup));
        let backup = dictionary(&value);

        // Restoring clears the table behind every key the backup carries, so an
        // empty list here would wipe what the user already had - and a
        // `sourceLists` of none replaces every list they subscribe to.
        for key in [
            "trackItems",
            "readingSessions",
            "updates",
            "settings",
            "sourceLists",
            "categories",
        ] {
            assert!(!backup.contains_key(key), "{key} should be left out");
        }

        // The one key the app insists on; without it the backup doesn't list.
        assert!(backup.get("date").and_then(plist::Value::as_date).is_some());
        // An imported backup the app deletes on its own schedule is a surprise.
        assert_eq!(
            backup.get("automatic").and_then(plist::Value::as_boolean),
            Some(false)
        );
    }

    #[test]
    fn an_unresolved_extension_takes_its_entries_with_it() {
        let mapped = extension("en.mangapill");
        let unmapped = extension("en.gone");
        let backup = backup(
            vec![Arc::clone(&mapped), Arc::clone(&unmapped)],
            vec![entry("/manga/1", &mapped), entry("/manga/2", &unmapped)],
        );

        let mut mapping = ExtensionMapping::new(Aidoku::ID);
        mapping
            .insert(Arc::clone(&mapped), Arc::clone(&mapped))
            .unwrap();

        let (value, report) = written(&backup, &mapping);

        assert_eq!(array(&value, "manga").len(), 1);
        assert_eq!(
            array(&value, "sources"),
            [plist::Value::from("en.mangapill")]
        );
        assert!(report.failed.is_empty());
        assert_eq!(
            report
                .skipped
                .iter()
                .map(|skipped| &skipped.extension)
                .collect::<Vec<_>>(),
            vec![unmapped.as_ref()]
        );
    }

    #[test]
    fn an_entry_outside_the_extension_table_is_reported_once() {
        let listed = extension("en.mangapill");
        let stray = extension("en.stray");
        // The reader built a second `Extension` for the same source instead of
        // sharing the one in the table, which is the bug this catches.
        let backup = backup(
            vec![Arc::clone(&listed)],
            vec![entry("/manga/1", &stray), entry("/manga/2", &stray)],
        );

        let (value, report) = written(&backup, &identity(&backup));

        assert!(dictionary(&value).get("manga").is_none());
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].extension, *stray);
    }

    #[test]
    fn a_source_without_an_id_is_a_failure_rather_than_a_row() {
        let source = extension("en.mangapill");
        let mut nameless = source.as_ref().clone();
        nameless.external_id = String::new();

        let backup = backup(vec![Arc::clone(&source)], vec![entry("/manga/1", &source)]);
        let mut mapping = ExtensionMapping::new(Aidoku::ID);
        mapping
            .insert(Arc::clone(&source), Arc::new(nameless))
            .unwrap();

        let (value, report) = written(&backup, &mapping);

        assert!(dictionary(&value).get("manga").is_none());
        assert!(dictionary(&value).get("sources").is_none());
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].extension, *source);
    }

    #[test]
    fn two_extensions_onto_one_source_leave_one_series() {
        let english = extension("en.mangadex");
        let spanish = extension("es.mangadex");
        let target = extension("multi.mangadex");

        let backup = backup(
            vec![Arc::clone(&english), Arc::clone(&spanish)],
            vec![entry("/manga/1", &english), entry("/manga/1", &spanish)],
        );

        let mut mapping = ExtensionMapping::new(Aidoku::ID);
        mapping
            .insert(Arc::clone(&english), Arc::clone(&target))
            .unwrap();
        mapping
            .insert(Arc::clone(&spanish), Arc::clone(&target))
            .unwrap();

        let (value, report) = written(&backup, &mapping);

        assert_eq!(report, WriteReport::default());
        assert_eq!(array(&value, "manga").len(), 1);
        assert_eq!(array(&value, "library").len(), 1);
        assert_eq!(
            array(&value, "sources"),
            [plist::Value::from("multi.mangadex")]
        );
    }

    #[test]
    fn page_progress_is_narrowed_to_what_swift_can_hold() {
        let source = extension("en.mangapill");
        let mut chapter = chapter("/chapters/1");
        chapter.last_page_read = i32::MAX;

        let mut entry = entry("/manga/1", &source);
        entry.chapters = vec![chapter];
        let backup = backup(vec![Arc::clone(&source)], vec![entry]);

        let (value, _) = written(&backup, &identity(&backup));
        let row = &array(&value, "history")[0];

        // `Int16(_:)` traps rather than wraps, so anything larger would take the
        // app down mid-restore.
        assert_eq!(
            field(row, "progress").as_signed_integer(),
            Some(i64::from(i16::MAX))
        );
    }

    #[test]
    fn chapters_take_the_language_the_entry_was_read_in() {
        let source = extension("en.mangadex");
        let target = extension("multi.mangadex");

        let mut entry = entry("/manga/1", &source);
        entry.chapters = vec![chapter("/chapters/1")];
        let backup = backup(vec![Arc::clone(&source)], vec![entry]);

        let mut mapping = ExtensionMapping::new(Aidoku::ID);
        mapping
            .insert(Arc::clone(&source), Arc::clone(&target))
            .unwrap();

        let (value, _) = written(&backup, &mapping);
        let row = &array(&value, "chapters")[0];

        assert_eq!(field(row, "lang").as_string(), Some("en"));
        assert_eq!(field(row, "sourceId").as_string(), Some("multi.mangadex"));
    }

    #[test]
    fn a_library_row_names_the_categories_that_survived() {
        let source = extension("en.mangapill");
        let mut entry = entry("/manga/1", &source);
        entry.categories = vec![
            CategoryID("1".to_string()),
            // A second id for the same title, and one the backup never listed.
            CategoryID("2".to_string()),
            CategoryID("3".to_string()),
        ];

        let mut backup = backup(vec![Arc::clone(&source)], vec![entry]);
        backup.categories = vec![
            Category {
                id: CategoryID("1".to_string()),
                name: "Reading".to_string(),
                last_modified: Timestamp::UNIX_EPOCH,
            },
            Category {
                id: CategoryID("2".to_string()),
                name: "Reading".to_string(),
                last_modified: Timestamp::UNIX_EPOCH,
            },
        ];

        let (value, _) = written(&backup, &identity(&backup));

        // Aidoku looks a category up by title and keeps the first of a pair
        // that share one, so writing both would leave a row nothing points at.
        assert_eq!(array(&value, "categories").len(), 1);
        assert_eq!(
            field(&array(&value, "library")[0], "categories")
                .as_array()
                .map(Vec::as_slice),
            Some([plist::Value::from("Reading")].as_slice())
        );
    }

    #[test]
    fn dates_the_ir_never_carried_fall_back_to_the_backup_s_own() {
        let source = extension("en.mangapill");
        let mut entry = entry("/manga/1", &source);
        entry.date_added = None;
        let backup = backup(vec![Arc::clone(&source)], vec![entry]);

        let (raw, _) = assemble(&backup, &identity(&backup), timestamp(NOW));

        assert_eq!(raw.library[0].date_added, date_or_epoch(timestamp(NOW)));
        // Not optional in the app's decoder, so they have to say something.
        assert_eq!(raw.library[0].last_opened, date_or_epoch(timestamp(NOW)));
        assert_eq!(raw.library[0].last_updated, date_or_epoch(timestamp(NOW)));
        assert_eq!(raw.library[0].last_read, None);
    }

    #[test]
    fn a_date_before_year_0000_degrades_to_the_epoch() {
        let source = extension("en.mangapill");
        let mut entry = entry("/manga/1", &source);
        // -62167219201 is the first second where `date()` returns None.
        entry.date_added = Timestamp::from_second(-62_167_219_201).ok();
        let backup = backup(vec![Arc::clone(&source)], vec![entry]);

        let (raw, _) = assemble(&backup, &identity(&backup), timestamp(NOW));

        // The pre-epoch date fell back to the epoch rather than panicking.
        assert_eq!(raw.library[0].date_added, date_or_epoch(timestamp(0)),);
        assert_eq!(raw.library[0].last_read, None);
    }

    #[test]
    fn an_unknown_source_language_falls_back_to_the_target() {
        let mangadex = extension("en.mangadex");
        let mut untyped = mangadex.as_ref().clone();
        untyped.lang = Language::Unknown;
        let source = Arc::new(untyped);

        let mut entry = entry("/manga/1", &source);
        entry.chapters = vec![chapter("/chapters/1")];
        let backup = backup(vec![Arc::clone(&source)], vec![entry]);

        let mut mapping = ExtensionMapping::new(Aidoku::ID);
        mapping
            .insert(Arc::clone(&source), Arc::clone(&mangadex))
            .unwrap();

        let (value, _) = written(&backup, &mapping);
        let row = &array(&value, "chapters")[0];

        // Unknown → falls back to target (en) not "all".
        assert_eq!(field(row, "lang").as_string(), Some("en"));
    }

    #[test]
    fn both_sides_unknown_or_all_write_empty_chapter_lang() {
        let untyped_source = {
            let mut src = extension("en.mangadex").as_ref().clone();
            src.lang = Language::Unknown;
            Arc::new(src)
        };
        let all_target = extension("multi.mangadex");

        let mut entry = entry("/manga/1", &untyped_source);
        entry.chapters = vec![chapter("/chapters/1")];
        let backup = backup(vec![Arc::clone(&untyped_source)], vec![entry]);

        let mut mapping = ExtensionMapping::new(Aidoku::ID);
        mapping
            .insert(Arc::clone(&untyped_source), Arc::clone(&all_target))
            .unwrap();

        let (value, _) = written(&backup, &mapping);
        let row = &array(&value, "chapters")[0];

        // Both sides say nothing specific → empty string.
        assert_eq!(field(row, "lang").as_string(), Some(""));
    }
}
