use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::sync::Arc;

use converter_core::extensions::lang::Language;
use converter_core::extensions::types::{Extension, ExtensionRepositoryInfo};
use converter_core::io::reader::{BackupReader, ReadReport};
use converter_core::io::{ReadError, SkippedExtension};
use converter_core::ir::types::{
    Backup, Category, CategoryID, Chapter, History, Manga, MangaStatus,
};
use flate2::read::GzDecoder;
use jiff::Timestamp;
use prost::Message as _;

use crate::Tachiyomi;
use crate::protos;

/// Entries the user side-loaded as files.
const LOCAL_SOURCE_ID: i64 = 0;
/// SY's stand-in source for merged entries.
const MERGED_SOURCE_ID: i64 = 6969;
/// A backup is protobuf, not media: even a large library is a few tens of megabytes decompressed,
/// so anything past this is a decompression bomb rather than someone's collection.
const MAX_DECOMPRESSED: usize = 512 * 1024 * 1024;

pub struct TachiReader {}

impl TachiReader {
    pub const fn new() -> Self {
        Self {}
    }
}

impl Default for TachiReader {
    fn default() -> Self {
        Self::new()
    }
}

impl BackupReader for TachiReader {
    converter_core::format_id!(Tachiyomi);

    fn read_backup(&self, bytes: &[u8]) -> Result<(Backup, ReadReport), ReadError> {
        let raw = decompress(bytes)?;
        let wire = protos::Backup::decode(raw.as_ref())
            .map_err(|error| ReadError::Malformed(error.to_string()))?;
        Ok(assemble(wire))
    }
}

/// A `.tachibk` (and a `.proto.gz`) is gzipped; anything already decoded passes through.
fn decompress(bytes: &[u8]) -> Result<Cow<'_, [u8]>, ReadError> {
    decompress_within(bytes, MAX_DECOMPRESSED)
}

/// Split out from [`decompress`] so a test can pass a limit it can actually reach.
fn decompress_within(bytes: &[u8], max: usize) -> Result<Cow<'_, [u8]>, ReadError> {
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        return Ok(Cow::Borrowed(bytes));
    }

    // The input is a file the user picked, so a corrupt or hostile one would otherwise expand
    // until allocation fails and takes the process down. Reading one byte past the cap is what
    // tells the two cases apart: a backup that stops short of it decompressed cleanly.
    let mut out = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX);
    let read = GzDecoder::new(bytes)
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|error| ReadError::Decompress(error.to_string()))?;
    if read > max {
        return Err(ReadError::Decompress(format!(
            "backup expands past the {max} byte limit"
        )));
    }
    Ok(Cow::Owned(out))
}

fn assemble(wire: protos::Backup) -> (Backup, ReadReport) {
    let mut sources = Sources::new(wire.backup_sources);

    let entries: Vec<Manga> = wire
        .backup_manga
        .into_iter()
        .filter_map(|manga| {
            let extension = sources.resolve(manga.source)?;
            Some(entry(manga, extension))
        })
        .collect();

    let categories = wire.backup_categories.into_iter().map(category).collect();
    let (extensions, skipped) = sources.finish();

    let backup = Backup {
        reader: Tachiyomi::ID,
        // The backup message carries no version of its own. Which fork wrote it is implied by
        // the proto numbers present, not recorded anywhere we could read back.
        version: None,
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

/// Hands out exactly one [`Arc<Extension>`] per Tachiyomi source id.
struct Sources {
    /// Names from `backupSources`, kept for ids we only meet later on an entry.
    names: HashMap<i64, String>,
    /// Insertion order, so the IR doesn't reshuffle between runs the way a `HashMap` would.
    order: Vec<i64>,
    by_id: HashMap<i64, Arc<Extension>>,
    skipped: Vec<SkippedExtension>,
    reported: HashSet<i64>,
}

impl Sources {
    fn new(wire: Vec<protos::BackupSource>) -> Self {
        let mut sources = Self {
            names: HashMap::new(),
            order: Vec::new(),
            by_id: HashMap::new(),
            skipped: Vec::new(),
            reported: HashSet::new(),
        };

        // Registering in `backupSources` order, not `names` key order: the app writes sources in
        // the order its entries first mention them, and a `HashMap`'s keys come back in a
        // different arbitrary order every run, which would reshuffle `Backup::extensions` between
        // two reads of the same file. Names go in first because `extension_for` reads them.
        let mut listed = Vec::with_capacity(wire.len());
        for source in wire {
            listed.push(source.source_id);
            sources
                .names
                .insert(source.source_id, source.name.unwrap_or_default());
        }

        for id in listed {
            if synthetic_reason(id).is_none() {
                sources.register(id);
            }
        }

        sources
    }

    /// `None` when the id has no installable extension behind it, so the caller drops the entry.
    fn resolve(&mut self, id: i64) -> Option<Arc<Extension>> {
        if let Some(reason) = synthetic_reason(id) {
            self.report(id, reason);
            return None;
        }
        Some(self.register(id))
    }

    fn register(&mut self, id: i64) -> Arc<Extension> {
        if let Some(existing) = self.by_id.get(&id) {
            return Arc::clone(existing);
        }

        let extension = Arc::new(self.extension_for(id));
        self.order.push(id);
        self.by_id.insert(id, Arc::clone(&extension));
        extension
    }

    fn report(&mut self, id: i64, reason: &str) {
        if !self.reported.insert(id) {
            return;
        }
        self.skipped.push(SkippedExtension {
            reason: reason.to_string(),
            extension: self.extension_for(id),
        });
    }

    fn extension_for(&self, id: i64) -> Extension {
        Extension {
            reader: Tachiyomi::ID,
            name: self.names.get(&id).cloned().unwrap_or_default(),
            external_id: id.to_string(),
            url: String::new(),
            // `backupSources` is only a name and an id. The id is an md5 of name/lang/version,
            // so the language is in there but not recoverable, and claiming `All` would cost
            // every candidate the multilingual penalty for something the backup never said.
            lang: Language::Unknown,
            repo: ExtensionRepositoryInfo::new(Tachiyomi::ID, None, None, None, None),
        }
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

/// Sources the app synthesises: there is no extension to install for either, so an entry behind
/// one cannot be re-pointed at another app.
const fn synthetic_reason(id: i64) -> Option<&'static str> {
    match id {
        LOCAL_SOURCE_ID => Some("stored locally as files rather than fetched from a source"),
        MERGED_SOURCE_ID => {
            Some("a TachiyomiSY merge, which is a view over other entries rather than a source")
        }
        _ => None,
    }
}

fn entry(wire: protos::BackupManga, extension: Arc<Extension>) -> Manga {
    let protos::BackupManga {
        url,
        title,
        artist,
        author,
        description,
        genre,
        status,
        thumbnail_url,
        date_added,
        chapters,
        categories,
        favorite,
        history,
        ..
    } = wire;

    // Tachiyomi keys a read date to a chapter by URL, exactly as `MangaRestorer.restoreHistory`
    // looks it up, so the join has to happen here: the IR wants the date on the chapter whenever
    // the chapter survived, and only falls back to `Manga::history` for the ones that did not.
    let read_at: HashMap<&str, i64> = history
        .iter()
        .map(|entry| (entry.url.as_str(), entry.last_read))
        .collect();

    let chapters: Vec<Chapter> = chapters
        .into_iter()
        .map(|chapter_wire| chapter(chapter_wire, &read_at))
        .collect();

    let covered: HashSet<&str> = chapters.iter().map(|chapter| chapter.id.as_str()).collect();
    let orphaned = history
        .iter()
        .filter(|entry| !covered.contains(entry.url.as_str()))
        .filter_map(|entry| {
            Some(History {
                // Tachiyomi's history rows carry a URL and nothing else identifying.
                chapter_id: None,
                url: non_empty(Some(entry.url.clone())),
                // A history row whose date we cannot represent carries nothing else, so drop it.
                last_read: timestamp(Some(entry.last_read))?,
            })
        })
        .collect();

    Manga {
        id: url,
        title: title.unwrap_or_default(),
        extension,
        status: manga_status(status.unwrap_or_default()),
        // Tachiyomi tracks NSFW per source, not per entry, and the backup carries neither.
        nsfw: false,
        // `BackupManga.favorite` defaults to true in Kotlin: a backed-up entry is in the library
        // unless it says otherwise.
        favorite: favorite.unwrap_or(true),
        author: non_empty(author),
        artist: non_empty(artist),
        description: non_empty(description),
        chapters,
        history: orphaned,
        tags: genre,
        categories: categories
            .into_iter()
            .map(|order| CategoryID(order.to_string()))
            .collect(),
        cover_url: non_empty(thumbnail_url),
        date_added: timestamp(date_added),
    }
}

fn chapter(wire: protos::BackupChapter, read_at: &HashMap<&str, i64>) -> Chapter {
    let last_read = read_at.get(wire.url.as_str()).copied();

    Chapter {
        title: wire.name,
        read: wire.read.unwrap_or_default(),
        last_page_read: page_index(wire.last_page_read.unwrap_or_default()),
        scanlator: non_empty(wire.scanlator),
        date_fetched: timestamp(wire.date_fetch),
        date_uploaded: timestamp(wire.date_upload),
        chapter_number: wire.chapter_number.map(f64::from),
        last_modified: timestamp(wire.last_modified_at),
        last_read: timestamp(last_read),
        id: wire.url,
    }
}

fn category(wire: protos::BackupCategory) -> Category {
    Category {
        name: wire.name,
        // Entries reference a category by its `order`, not its `id`: `MangaBackupCreator` writes
        // `categories.map { it.order }` and `MangaRestorer` looks them up the same way. Keying on
        // `id` here would quietly file every entry under the wrong category.
        id: CategoryID(wire.order.unwrap_or_default().to_string()),
        last_modified: timestamp(wire.last_modified_at).unwrap_or(Timestamp::UNIX_EPOCH),
    }
}

/// Mirrors `SManga`'s status constants, which the backup stores raw.
const fn manga_status(value: i32) -> MangaStatus {
    match value {
        1 => MangaStatus::Ongoing,
        2 => MangaStatus::Completed,
        3 => MangaStatus::Licensed,
        4 => MangaStatus::PublishingFinished,
        5 => MangaStatus::Cancelled,
        6 => MangaStatus::OnHiatus,
        _ => MangaStatus::Unknown,
    }
}

/// Tachiyomi writes 0 for "never", which is a real instant to `jiff` but not a real date here.
fn timestamp(millis: Option<i64>) -> Option<Timestamp> {
    match millis {
        Some(millis) if millis != 0 => Timestamp::from_millisecond(millis).ok(),
        _ => None,
    }
}

/// A page index is a `Long` upstream and an `i32` in the IR. Clamp rather than wrap, and treat a
/// negative as the start: Tachiyomi has no meaning for one.
fn page_index(value: i64) -> i32 {
    i32::try_from(value.clamp(0, i64::from(i32::MAX))).unwrap_or(i32::MAX)
}

/// Tachiyomi writes `""` about as often as it omits a field; the IR means "absent" by `None`.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;

    fn source(id: i64, name: &str) -> protos::BackupSource {
        protos::BackupSource {
            name: Some(name.to_string()),
            source_id: id,
        }
    }

    fn manga(source_id: i64, url: &str) -> protos::BackupManga {
        protos::BackupManga {
            source: source_id,
            url: url.to_string(),
            title: Some(url.to_string()),
            ..Default::default()
        }
    }

    fn chapter_wire(url: &str) -> protos::BackupChapter {
        protos::BackupChapter {
            url: url.to_string(),
            name: url.to_string(),
            ..Default::default()
        }
    }

    fn read(wire: &protos::Backup) -> (Backup, ReadReport) {
        let bytes = wire.encode_to_vec();
        TachiReader::new()
            .read_backup(&bytes)
            .expect("a backup we just encoded should read back")
    }

    #[test]
    fn sources_become_extensions_keyed_by_id() {
        let wire = protos::Backup {
            backup_sources: vec![source(9001, "Batoto")],
            backup_manga: vec![manga(9001, "/series/1")],
            ..Default::default()
        };

        let (backup, report) = read(&wire);

        assert_eq!(backup.reader, Tachiyomi::ID);
        assert_eq!(backup.extensions.len(), 1);
        assert_eq!(backup.extensions[0].name, "Batoto");
        // Keiyoushi scores a matching `external_id` at 1.0, so the numeric id is the part that
        // has to survive the trip.
        assert_eq!(backup.extensions[0].external_id, "9001");
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn entries_share_one_arc_with_the_extension_list() {
        // The invariant `ExtensionMapping` depends on: equal-looking `Extension`s hash the same
        // but resolve to nothing, so this has to be pointer identity, not equality.
        let wire = protos::Backup {
            backup_sources: vec![source(9001, "Batoto")],
            backup_manga: vec![manga(9001, "/a"), manga(9001, "/b")],
            ..Default::default()
        };

        let (backup, _) = read(&wire);

        assert_eq!(backup.entries.len(), 2);
        for entry in &backup.entries {
            assert!(Arc::ptr_eq(&entry.extension, &backup.extensions[0]));
        }
    }

    #[test]
    fn an_entry_whose_source_was_never_listed_still_shares_an_arc() {
        // Older backups omit `backupSources` entirely. Synthesising a per-entry `Extension` here
        // is exactly the reader bug `PruneReport::unknown` exists to catch.
        let wire = protos::Backup {
            backup_manga: vec![manga(4242, "/a"), manga(4242, "/b")],
            ..Default::default()
        };

        let (backup, _) = read(&wire);

        assert_eq!(backup.extensions.len(), 1);
        assert_eq!(backup.extensions[0].external_id, "4242");
        assert!(backup.extensions[0].name.is_empty());
        for entry in &backup.entries {
            assert!(Arc::ptr_eq(&entry.extension, &backup.extensions[0]));
        }
    }

    #[test]
    fn local_and_merged_entries_are_skipped_and_reported_once() {
        let wire = protos::Backup {
            backup_sources: vec![
                source(LOCAL_SOURCE_ID, "Local source"),
                source(9001, "Batoto"),
            ],
            backup_manga: vec![
                manga(LOCAL_SOURCE_ID, "/local-a"),
                manga(LOCAL_SOURCE_ID, "/local-b"),
                manga(MERGED_SOURCE_ID, "/merged"),
                manga(9001, "/real"),
            ],
            ..Default::default()
        };

        let (backup, report) = read(&wire);

        assert_eq!(backup.entries.len(), 1);
        assert_eq!(backup.entries[0].id, "/real");
        // Two local entries, one report.
        assert_eq!(report.skipped.len(), 2);
        let ids: Vec<&str> = report
            .skipped
            .iter()
            .map(|skip| skip.extension.external_id.as_str())
            .collect();
        assert!(ids.contains(&"0"));
        assert!(ids.contains(&"6969"));
        // Neither belongs in the mapping: there is nothing to install for either.
        assert_eq!(backup.extensions.len(), 1);
        assert_eq!(backup.extensions[0].external_id, "9001");
    }

    #[test]
    fn history_lands_on_the_chapter_when_the_chapter_survived() {
        let mut entry = manga(9001, "/series");
        entry.chapters = vec![chapter_wire("/series/c1")];
        entry.history = vec![protos::BackupHistory {
            url: "/series/c1".to_string(),
            last_read: 1_700_000_000_000,
            read_duration: None,
        }];

        let wire = protos::Backup {
            backup_manga: vec![entry],
            ..Default::default()
        };
        let (backup, _) = read(&wire);
        let entry = &backup.entries[0];

        assert_eq!(
            entry.chapters[0].last_read,
            Some(Timestamp::from_millisecond(1_700_000_000_000).unwrap())
        );
        // The IR reserves `Manga::history` for chapters that are *not* in the list.
        assert!(entry.history.is_empty());
    }

    #[test]
    fn history_for_a_missing_chapter_falls_back_to_the_history_list() {
        let mut entry = manga(9001, "/series");
        entry.chapters = vec![chapter_wire("/series/c1")];
        entry.history = vec![protos::BackupHistory {
            url: "/series/gone".to_string(),
            last_read: 1_700_000_000_000,
            read_duration: None,
        }];

        let wire = protos::Backup {
            backup_manga: vec![entry],
            ..Default::default()
        };
        let (backup, _) = read(&wire);
        let entry = &backup.entries[0];

        assert!(entry.chapters[0].last_read.is_none());
        assert_eq!(entry.history.len(), 1);
        assert_eq!(entry.history[0].url.as_deref(), Some("/series/gone"));
        assert_eq!(entry.history[0].chapter_id, None);
    }

    #[test]
    fn categories_are_keyed_by_order_not_id() {
        // `MangaBackupCreator` writes `categories.map { it.order }`. An `id` of 77 next to an
        // `order` of 1 is exactly the shape that catches a reader keying on the wrong field.
        let mut entry = manga(9001, "/series");
        entry.categories = vec![1];

        let wire = protos::Backup {
            backup_categories: vec![protos::BackupCategory {
                name: "Reading".to_string(),
                order: Some(1),
                id: Some(77),
                last_modified_at: Some(1_700_000_000_000),
                ..Default::default()
            }],
            backup_manga: vec![entry],
            ..Default::default()
        };

        let (backup, _) = read(&wire);

        assert_eq!(backup.categories[0].id, CategoryID("1".to_string()));
        assert_eq!(
            backup.entries[0].categories,
            vec![CategoryID("1".to_string())]
        );
    }

    #[test]
    fn statuses_follow_smanga_constants() {
        for (raw, expected) in [
            (0, MangaStatus::Unknown),
            (1, MangaStatus::Ongoing),
            (2, MangaStatus::Completed),
            (3, MangaStatus::Licensed),
            (4, MangaStatus::PublishingFinished),
            (5, MangaStatus::Cancelled),
            (6, MangaStatus::OnHiatus),
            (99, MangaStatus::Unknown),
        ] {
            assert_eq!(manga_status(raw), expected, "status {raw}");
        }
    }

    #[test]
    fn a_gzipped_backup_reads_the_same_as_a_bare_one() {
        // `read_backup` never sees the filename, so the container is sniffed.
        let wire = protos::Backup {
            backup_sources: vec![source(9001, "Batoto")],
            backup_manga: vec![manga(9001, "/series")],
            ..Default::default()
        };
        let bare = wire.encode_to_vec();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&bare).unwrap();
        let gzipped = encoder.finish().unwrap();

        let reader = TachiReader::new();
        let (from_bare, _) = reader.read_backup(&bare).unwrap();
        let (from_gzipped, _) = reader.read_backup(&gzipped).unwrap();

        assert_eq!(from_bare, from_gzipped);
    }

    #[test]
    fn extensions_keep_the_order_backup_sources_listed_them_in() {
        // `Sources::order` used to be seeded from a `HashMap`'s keys, which come back in a
        // different arbitrary order every run: two reads of one file disagreed on
        // `Backup::extensions`, and any writer emitting sources in list order inherited that.
        let listed: Vec<i64> = (1..=12).collect();
        let wire = protos::Backup {
            backup_sources: listed.iter().map(|id| source(*id, "Source")).collect(),
            ..Default::default()
        };
        let expected: Vec<String> = listed.iter().map(i64::to_string).collect();

        for _ in 0..8 {
            let (backup, _) = read(&wire);
            let seen: Vec<String> = backup
                .extensions
                .iter()
                .map(|extension| extension.external_id.clone())
                .collect();
            assert_eq!(seen, expected);
        }
    }

    #[test]
    fn an_entry_only_extends_the_order_it_does_not_reshuffle_it() {
        // Ids met for the first time on an entry land after the listed ones, in entry order.
        let wire = protos::Backup {
            backup_sources: vec![source(3, "C"), source(1, "A")],
            backup_manga: vec![manga(7, "/a"), manga(1, "/b"), manga(5, "/c")],
            ..Default::default()
        };

        let (backup, _) = read(&wire);

        let seen: Vec<&str> = backup
            .extensions
            .iter()
            .map(|extension| extension.external_id.as_str())
            .collect();
        assert_eq!(seen, ["3", "1", "7", "5"]);
    }

    #[test]
    fn a_decompression_bomb_errors_instead_of_exhausting_memory() {
        // Unbounded, this is the input that ends the process rather than the read.
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&vec![0u8; 64 * 1024]).unwrap();
        let bomb = encoder.finish().unwrap();

        let error = decompress_within(&bomb, 1024).expect_err("should refuse to expand past 1KiB");

        assert!(matches!(error, ReadError::Decompress(_)), "{error:?}");
    }

    #[test]
    fn a_backup_that_fits_the_limit_still_decompresses_whole() {
        // The cap must not truncate: exactly-at-the-limit is a legitimate backup, not a bomb.
        let payload = vec![7u8; 1024];
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&payload).unwrap();
        let gzipped = encoder.finish().unwrap();

        let out = decompress_within(&gzipped, 1024).expect("1KiB should fit a 1KiB limit");

        assert_eq!(out.as_ref(), payload.as_slice());
    }

    #[test]
    fn a_source_without_a_language_reads_as_unknown_not_all() {
        // `backupSources` has no language field. `All` is a claim ("serves every language") that
        // costs the source a scoring penalty against every candidate; `Unknown` is the absence.
        let wire = protos::Backup {
            backup_sources: vec![source(9001, "Batoto")],
            ..Default::default()
        };

        let (backup, _) = read(&wire);

        assert_eq!(backup.extensions[0].lang, Language::Unknown);
    }

    #[test]
    fn absent_and_empty_fields_both_read_as_none() {
        let mut entry = manga(9001, "/series");
        entry.author = Some(String::new());
        entry.artist = None;
        entry.thumbnail_url = Some(String::new());
        entry.date_added = Some(0);

        let wire = protos::Backup {
            backup_manga: vec![entry],
            ..Default::default()
        };
        let (backup, _) = read(&wire);
        let entry = &backup.entries[0];

        assert_eq!(entry.author, None);
        assert_eq!(entry.artist, None);
        assert_eq!(entry.cover_url, None);
        // 0 is Tachiyomi's "never", not 1970.
        assert_eq!(entry.date_added, None);
        // Kotlin defaults `favorite` to true, so an omitted field means "in the library".
        assert!(entry.favorite);
    }

    #[test]
    fn a_page_index_too_large_for_the_ir_clamps_instead_of_wrapping() {
        let mut chapter = chapter_wire("/c1");
        chapter.last_page_read = Some(i64::from(i32::MAX) + 1);
        let mut entry = manga(9001, "/series");
        entry.chapters = vec![chapter];

        let wire = protos::Backup {
            backup_manga: vec![entry],
            ..Default::default()
        };
        let (backup, _) = read(&wire);

        assert_eq!(backup.entries[0].chapters[0].last_page_read, i32::MAX);
        assert_eq!(page_index(-5), 0);
    }

    #[test]
    fn bytes_that_are_not_a_backup_are_rejected() {
        let reader = TachiReader::new();

        // Not protobuf at all.
        let error = reader.read_backup(&[0xff, 0xff, 0xff, 0xff]).unwrap_err();
        assert!(matches!(error, ReadError::Malformed(_)), "{error:?}");

        // Claims to be gzip, then is not.
        let error = reader.read_backup(&[0x1f, 0x8b, 0x00, 0x00]).unwrap_err();
        assert!(matches!(error, ReadError::Decompress(_)), "{error:?}");
    }

    /// Reads a real backup end to end. Ignored because it needs a file this repo does not ship;
    /// point `TACHIBK` at one, or drop a `backup.tachibk` in the workspace root.
    ///
    /// ```sh
    /// cargo test -p converter-tachi -- --ignored real_backup --nocapture
    /// ```
    #[ignore = "needs a real backup on disk"]
    #[test]
    // The counts are the point of running it by hand; `--nocapture` is how you see them.
    #[allow(clippy::print_stdout)]
    fn real_backup_round_trips() {
        let path = std::env::var("TACHIBK").unwrap_or_else(|_| "../../backup.tachibk".to_string());
        let bytes = std::fs::read(&path).expect("set TACHIBK to a .tachibk");
        let (backup, report) = TachiReader::new().read_backup(&bytes).unwrap();

        println!(
            "{} entries, {} extensions, {} categories, {} sources skipped",
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

        // The `order`-not-`id` question, against real data: a backup keyed the other way leaves
        // these dangling. Assert the check is not vacuous first.
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
    }
}
