use super::{
    Backup, BackupReader, BackupWriter, Category, Chapter, History, IRError, Manga, MangaStatus,
    Result, WriteTracker,
};
use crate::blacklist;
use crate::matching::{Language, Matcher, SourceData};
use crate::reader::ReaderId;
use crate::report::{DroppedManga, ReadReport, WriteReport};
use chrono::{DateTime, NaiveDateTime, Utc};
use plist::{Dictionary, Value as PlistValue};
use serde_json::{Map, Number, Value as JsonValue};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::time::SystemTime;
use tracing::{debug, info, trace, warn};

pub struct AidokuConverter;

type JsonObject = Map<String, JsonValue>;
type MangaKey = (String, String);
type ChapterKey = (String, String, String);

const PLIST_MAGIC: &[u8] = b"bplist00";
const AIDOKU_BACKUP_VERSION: &str = "0.8.3";

#[derive(Clone, Copy, Debug, Default)]
struct ReadState {
    completed: bool,
    progress: i32,
    last_read: i64,
}

impl BackupReader for AidokuConverter {
    fn read_backup(bytes: &[u8]) -> Result<(Backup, ReadReport)> {
        let backup = parse_backup_bytes(bytes)?;
        let root = object(&backup, "Aidoku backup root")?;
        let manga_details = manga_details_by_key(root);
        let chapters = chapters_by_manga(root);
        let histories = history_by_manga(root);
        let latest_history = latest_history_by_chapter(root)?;

        let mut manga = Vec::new();
        let mut dropped = Vec::new();
        for entry in array_field(root, "library") {
            if let Some(source_id) = string_field(entry, "sourceId")
                && let Some(reason) = blacklist::reason(ReaderId::Aidoku, &source_id)
            {
                dropped.push(dropped_for_entry(
                    entry,
                    &source_id,
                    &manga_details,
                    &chapters,
                    reason,
                ));
                continue;
            }
            manga.push(library_entry_to_ir(
                entry,
                &manga_details,
                &chapters,
                &histories,
                &latest_history,
            )?);
        }

        Ok((
            Backup {
                reader: ReaderId::Aidoku,
                manga,
                categories: categories_to_ir(root),
            },
            ReadReport { dropped },
        ))
    }
}

impl BackupWriter for AidokuConverter {
    fn write_backup(backup: &Backup, matcher: &Matcher) -> Result<(Vec<u8>, WriteReport)> {
        debug!(
            manga = backup.manga.len(),
            categories = backup.categories.len(),
            "Building Aidoku backup"
        );

        let mut source_ids = HashSet::new();
        let mut source_list_urls = HashSet::new();
        let mut library = Vec::new();
        let mut manga_items = Vec::new();
        let mut chapter_items = Vec::new();
        let mut history_items = Vec::new();
        let mut non_favorite_manga = 0usize;
        let mut tracker = WriteTracker::default();

        for manga in &backup.manga {
            trace!(
                title = %manga.title,
                chapters = manga.chapters.len(),
                history_entries = manga.history.len(),
                "Converting manga to Aidoku"
            );

            if tracker.note_if_failed(manga) {
                warn!(
                    title = %manga.title,
                    source = manga.source.name,
                    "skipping manga because its Aidoku source previously failed to match"
                );
                continue;
            }

            let source = match aidoku_source_for_manga(manga, matcher) {
                Ok(source) => source,
                Err(err) => {
                    warn!(
                        title = %manga.title,
                        source = manga.source.name,
                        error = %err,
                        "failed to match source; skipping this and subsequent manga from the same source"
                    );
                    tracker.record_failure(manga, "no matching Aidoku source found");
                    continue;
                }
            };

            let source_id = aidoku_source_id(&source, &manga.title)?;

            tracker.record_mapping(manga, || {
                format!(
                    "{} ({})",
                    source.name.as_deref().unwrap_or(source_id.as_str()),
                    ReaderId::Aidoku
                )
            });

            if let Some(repo) = source.repo.as_ref()
                && let Some(base_url) = repo.base_url.as_ref()
            {
                source_list_urls.insert(base_url.clone());
            }

            source_ids.insert(source_id.clone());

            // Aidoku's library tracks membership by presence in the library array. We skip entries
            // for non-favorites to prevent history leaking into the user's library.
            if manga.favorite {
                library.push(library_entry_to_plist(manga, &source_id));
            } else {
                non_favorite_manga += 1;
            }
            manga_items.push(manga_to_plist(manga, &source_id));    // always track history

            let chapters_by_id = manga
                .chapters
                .iter()
                .map(|chapter| (chapter.id.as_str(), chapter))
                .collect::<HashMap<_, _>>();

            for (idx, chapter) in manga.chapters.iter().enumerate() {
                chapter_items.push(chapter_to_plist(chapter, manga, &source_id, idx));
            }

            for history in &manga.history {
                let chapter = chapters_by_id.get(history.url.as_str()).copied();
                history_items.push(history_to_plist(history, manga, &source_id, chapter));
            }

            let history_chapter_ids = manga
                .history
                .iter()
                .map(|history| history.url.as_str())
                .collect::<HashSet<_>>();

            for chapter in manga.chapters.iter().filter(|chapter| {
                !history_chapter_ids.contains(chapter.id.as_str())
                    && (chapter.read || chapter.last_page_read != 0)
            }) {
                let history = History {
                    url: chapter.id.clone(),
                    last_read: synthesized_history_time(manga, chapter),
                };
                history_items.push(history_to_plist(&history, manga, &source_id, Some(chapter)));
            }
        }

        let mut source_ids = source_ids.into_iter().collect::<Vec<_>>();
        source_ids.sort();
        let mut source_list_urls = source_list_urls.into_iter().collect::<Vec<_>>();
        source_list_urls.sort();
        info!(
            converted_manga = manga_items.len(),
            library_entries = library.len(),
            non_favorite_manga,
            skipped_manga = tracker.skipped_manga_count(),
            sources = source_ids.len(),
            source_lists = source_list_urls.len(),
            "Finished building Aidoku backup data"
        );

        let manga_count = manga_items.len();
        let chapter_count = chapter_items.len();
        let history_count = history_items.len();
        // Mirror the deduped category list actually written to the plist so the
        // reported count matches the output exactly.
        let category_count = categories_to_plist(backup).len();

        let mut root = Dictionary::new();
        root.insert("library".into(), PlistValue::Array(library));
        root.insert("history".into(), PlistValue::Array(history_items));
        root.insert("manga".into(), PlistValue::Array(manga_items));
        root.insert("chapters".into(), PlistValue::Array(chapter_items));
        root.insert("trackItems".into(), PlistValue::Array(Vec::new()));
        root.insert("readingSessions".into(), PlistValue::Array(Vec::new()));
        root.insert("updates".into(), PlistValue::Array(Vec::new()));
        root.insert(
            "categories".into(),
            PlistValue::Array(categories_to_plist(backup)),
        );
        root.insert(
            "sources".into(),
            PlistValue::Array(source_ids.into_iter().map(PlistValue::String).collect()),
        );
        root.insert(
            "sourceLists".into(),
            PlistValue::Array(
                source_list_urls
                    .into_iter()
                    .map(PlistValue::String)
                    .collect(),
            ),
        );
        root.insert("settings".into(), PlistValue::Dictionary(Dictionary::new()));
        root.insert(
            "date".into(),
            PlistValue::Date(plist::Date::from(SystemTime::now())),
        );
        root.insert("automatic".into(), PlistValue::Boolean(false));
        root.insert(
            "name".into(),
            PlistValue::String(format!("Converted from {}", backup.reader)),
        );
        root.insert(
            "version".into(),
            PlistValue::String(AIDOKU_BACKUP_VERSION.into()),
        );

        let (mappings, skipped_sources) = tracker.into_parts();
        let write_report = WriteReport {
            manga: manga_count,
            chapters: chapter_count,
            history: history_count,
            categories: category_count,
            mappings,
            skipped_sources,
        };

        let mut bytes = Vec::new();
        PlistValue::Dictionary(root)
            .to_writer_binary(&mut bytes)
            .map_err(|err| IRError::Serialize(format!("failed to write Aidoku plist: {err}")))?;

        debug!(bytes = bytes.len(), "Finished serializing Aidoku backup");
        Ok((bytes, write_report))
    }
}

fn parse_backup_bytes(bytes: &[u8]) -> Result<JsonValue> {
    if bytes.starts_with(PLIST_MAGIC) {
        let value = PlistValue::from_reader(Cursor::new(bytes))
            .map_err(|err| IRError::Parse(format!("failed to parse Aidoku binary plist: {err}")))?;
        return Ok(plist_to_json(value));
    }

    match serde_json::from_slice(bytes) {
        Ok(value) => Ok(value),
        Err(json_err) => {
            let plist_value = PlistValue::from_reader(Cursor::new(bytes)).map_err(|plist_err| {
                IRError::Parse(format!(
                    "failed to parse Aidoku backup as JSON ({json_err}) or plist ({plist_err})"
                ))
            })?;
            Ok(plist_to_json(plist_value))
        }
    }
}

fn plist_to_json(value: PlistValue) -> JsonValue {
    match value {
        PlistValue::Array(values) => {
            JsonValue::Array(values.into_iter().map(plist_to_json).collect())
        }
        PlistValue::Dictionary(values) => JsonValue::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, plist_to_json(value)))
                .collect(),
        ),
        PlistValue::Boolean(value) => JsonValue::Bool(value),
        PlistValue::Date(value) => {
            let time: SystemTime = value.into();
            JsonValue::Number(Number::from(DateTime::<Utc>::from(time).timestamp_millis()))
        }
        PlistValue::Integer(value) => value.as_signed().map_or(JsonValue::Null, |value| {
            JsonValue::Number(Number::from(value))
        }),
        PlistValue::Real(value) => {
            Number::from_f64(value).map_or(JsonValue::Null, JsonValue::Number)
        }
        PlistValue::String(value) => JsonValue::String(value),
        PlistValue::Uid(value) => JsonValue::Number(Number::from(value.get())),
        _ => JsonValue::Null,
    }
}

fn object<'a>(value: &'a JsonValue, context: &str) -> Result<&'a JsonObject> {
    value
        .as_object()
        .ok_or_else(|| IRError::Parse(format!("{context} must be an object")))
}

fn array_field<'a>(object: &'a JsonObject, key: &str) -> Vec<&'a JsonObject> {
    object
        .get(key)
        .and_then(JsonValue::as_array)
        .map(|values| values.iter().filter_map(JsonValue::as_object).collect())
        .unwrap_or_default()
}

fn categories_to_ir(root: &JsonObject) -> Vec<Category> {
    array_field(root, "categories")
        .iter()
        .enumerate()
        .filter_map(|(idx, category)| {
            let name = string_field(category, "title")?;
            let id = integer_field(category, "sort")
                .unwrap_or(idx as i64)
                .to_string();

            Some(Category {
                name,
                id,
                last_modified: 0,
            })
        })
        .collect()
}

fn manga_details_by_key(root: &JsonObject) -> HashMap<MangaKey, &JsonObject> {
    let mut details = HashMap::new();

    for manga in array_field(root, "manga") {
        if let Some(key) = optional_manga_key(manga) {
            details.entry(key).or_insert(manga);
        }
    }

    details
}

fn chapters_by_manga(root: &JsonObject) -> HashMap<MangaKey, Vec<&JsonObject>> {
    let mut chapters: HashMap<MangaKey, Vec<&JsonObject>> = HashMap::new();

    for chapter in array_field(root, "chapters") {
        if let Some(key) = optional_manga_key(chapter) {
            chapters.entry(key).or_default().push(chapter);
        }
    }

    chapters
}

fn history_by_manga(root: &JsonObject) -> HashMap<MangaKey, Vec<&JsonObject>> {
    let mut histories: HashMap<MangaKey, Vec<&JsonObject>> = HashMap::new();

    for history in array_field(root, "history") {
        if let Some(key) = optional_manga_key(history) {
            histories.entry(key).or_default().push(history);
        }
    }

    histories
}

fn latest_history_by_chapter(root: &JsonObject) -> Result<HashMap<ChapterKey, ReadState>> {
    let mut states = HashMap::new();

    for history in array_field(root, "history") {
        let source_id = required_string(history, "sourceId", "Aidoku history")?;
        let manga_id = required_string(history, "mangaId", "Aidoku history")?;
        let chapter_id = required_string(history, "chapterId", "Aidoku history")?;
        let last_read = date_millis_field(history, "dateRead").unwrap_or_default();
        let progress = integer_field(history, "progress")
            .unwrap_or_default()
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        let state = ReadState {
            completed: bool_field(history, "completed").unwrap_or_default(),
            progress,
            last_read,
        };

        states
            .entry((source_id, manga_id, chapter_id))
            .and_modify(|existing: &mut ReadState| {
                if state.last_read >= existing.last_read {
                    *existing = state;
                }
            })
            .or_insert(state);
    }

    Ok(states)
}

fn library_entry_to_ir(
    library_entry: &JsonObject,
    manga_details: &HashMap<MangaKey, &JsonObject>,
    chapters: &HashMap<MangaKey, Vec<&JsonObject>>,
    histories: &HashMap<MangaKey, Vec<&JsonObject>>,
    latest_history: &HashMap<ChapterKey, ReadState>,
) -> Result<Manga> {
    let source_id = required_string(library_entry, "sourceId", "Aidoku library entry")?;
    let manga_id = required_string(library_entry, "mangaId", "Aidoku library entry")?;
    let key = (source_id.clone(), manga_id.clone());
    let details = manga_details.get(&key).copied();
    let source = source_data_for(&source_id, details, chapters.get(&key));

    Ok(Manga {
        id: manga_id.clone(),
        source,
        title: details
            .and_then(|manga| string_field(manga, "title"))
            .unwrap_or_else(|| manga_id.clone()),
        author: details.and_then(|manga| string_field(manga, "author")),
        artist: details.and_then(|manga| string_field(manga, "artist")),
        desc: details.and_then(|manga| string_field(manga, "desc")),
        tags: details
            .map(|manga| string_vec_field(manga, "tags"))
            .unwrap_or_default(),
        cover_url: details.and_then(|manga| string_field(manga, "cover")),
        status: details
            .and_then(|manga| integer_field(manga, "status"))
            .map(MangaStatus::from_code)
            .unwrap_or_default(),
        nsfw: details
            .and_then(|manga| bool_field(manga, "nsfw"))
            .unwrap_or_default(),
        date_added: date_millis_field(library_entry, "dateAdded"),
        categories: string_vec_field(library_entry, "categories"),
        chapters: chapters
            .get(&key)
            .map(|chapters| {
                chapters
                    .iter()
                    .map(|chapter| chapter_to_ir(chapter, &source_id, &manga_id, latest_history))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default(),
        history: histories
            .get(&key)
            .map(|histories| {
                histories
                    .iter()
                    .map(|history| history_to_ir(history))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default(),
        favorite: true,
    })
}

fn dropped_for_entry(
    library_entry: &JsonObject,
    source_id: &str,
    manga_details: &HashMap<MangaKey, &JsonObject>,
    chapters: &HashMap<MangaKey, Vec<&JsonObject>>,
    reason: &str,
) -> DroppedManga {
    let manga_id = string_field(library_entry, "mangaId").unwrap_or_default();
    let key = (source_id.to_owned(), manga_id.clone());
    let title = manga_details
        .get(&key)
        .and_then(|manga| string_field(manga, "title"))
        .unwrap_or(manga_id);

    DroppedManga {
        title,
        reason: reason.to_string(),
        chapters: chapters.get(&key).map_or(0, Vec::len),
    }
}

fn chapter_to_ir(
    chapter: &JsonObject,
    source_id: &str,
    manga_id: &str,
    latest_history: &HashMap<ChapterKey, ReadState>,
) -> Result<Chapter> {
    let chapter_id = required_string(chapter, "id", "Aidoku chapter")?;
    let state = latest_history
        .get(&(
            source_id.to_owned(),
            manga_id.to_owned(),
            chapter_id.clone(),
        ))
        .copied()
        .unwrap_or_default();

    Ok(Chapter {
        id: chapter_id.clone(),
        title: string_field(chapter, "title").unwrap_or(chapter_id),
        scanlator: string_field(chapter, "scanlator").unwrap_or_default(),
        date_fetched: 0,
        date_uploaded: date_millis_field(chapter, "dateUploaded").unwrap_or_default(),
        chapter_number: float_field(chapter, "chapter").unwrap_or_default(),
        last_modified: 0,
        read: state.completed,
        last_page_read: state.progress,
    })
}

fn history_to_ir(history: &JsonObject) -> Result<History> {
    Ok(History {
        url: required_string(history, "chapterId", "Aidoku history")?,
        last_read: date_millis_field(history, "dateRead").unwrap_or_default(),
    })
}

fn source_data_for(
    source_id: &str,
    manga: Option<&JsonObject>,
    chapters: Option<&Vec<&JsonObject>>,
) -> SourceData {
    SourceData {
        reader: ReaderId::Aidoku,
        name: Some(source_name_from_id(source_id)),
        external_id: Some(source_id.to_owned()),
        url: source_base_url(manga, chapters),
        lang: language_from_source_id(source_id),
        repo: None,
    }
}

fn source_base_url(
    manga: Option<&JsonObject>,
    chapters: Option<&Vec<&JsonObject>>,
) -> Option<String> {
    manga
        .and_then(|manga| string_field(manga, "url"))
        .and_then(|url| origin_from_url(&url))
        .or_else(|| {
            chapters.and_then(|chapters| {
                chapters.iter().find_map(|chapter| {
                    string_field(chapter, "url").and_then(|url| origin_from_url(&url))
                })
            })
        })
}

fn origin_from_url(url: &str) -> Option<String> {
    let scheme_end = url.find("://")? + 3;
    let rest = &url[scheme_end..];
    let host_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());

    (host_len > 0).then(|| {
        url[..scheme_end + host_len]
            .trim_end_matches('/')
            .to_owned()
    })
}

fn source_name_from_id(source_id: &str) -> String {
    source_id
        .rsplit_once('.')
        .map_or(source_id, |(_, name)| name)
        .to_owned()
}

fn language_from_source_id(source_id: &str) -> Option<Language> {
    let prefix = source_id.split('.').next()?;
    (!prefix.is_empty()).then(|| Language::from_code(prefix))
}

fn optional_manga_key(object: &JsonObject) -> Option<MangaKey> {
    Some((
        string_field(object, "sourceId")?,
        string_field(object, "mangaId").or_else(|| string_field(object, "id"))?,
    ))
}

fn required_string(object: &JsonObject, key: &str, context: &str) -> Result<String> {
    string_field(object, key)
        .ok_or_else(|| IRError::Parse(format!("{context} is missing required field `{key}`")))
}

fn string_field(object: &JsonObject, key: &str) -> Option<String> {
    object.get(key).and_then(|value| match value {
        JsonValue::String(value) => Some(value.clone()),
        JsonValue::Number(value) => Some(value.to_string()),
        JsonValue::Bool(value) => Some(value.to_string()),
        _ => None,
    })
}

fn string_vec_field(object: &JsonObject, key: &str) -> Vec<String> {
    object
        .get(key)
        .and_then(JsonValue::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| match value {
                    JsonValue::String(value) => Some(value.clone()),
                    JsonValue::Number(value) => Some(value.to_string()),
                    JsonValue::Bool(value) => Some(value.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn integer_field(object: &JsonObject, key: &str) -> Option<i64> {
    object.get(key).and_then(|value| match value {
        JsonValue::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_f64().map(|value| value as i64)),
        JsonValue::String(value) => value.parse::<i64>().ok(),
        JsonValue::Bool(value) => Some(i64::from(*value)),
        _ => None,
    })
}

fn float_field(object: &JsonObject, key: &str) -> Option<f64> {
    object.get(key).and_then(|value| match value {
        JsonValue::Number(value) => value.as_f64(),
        JsonValue::String(value) => value.parse::<f64>().ok(),
        JsonValue::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        _ => None,
    })
}

fn bool_field(object: &JsonObject, key: &str) -> Option<bool> {
    object.get(key).and_then(|value| match value {
        JsonValue::Bool(value) => Some(*value),
        JsonValue::Number(value) => value.as_i64().map(|value| value != 0),
        JsonValue::String(value) => match value.as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

fn date_millis_field(object: &JsonObject, key: &str) -> Option<i64> {
    object.get(key).and_then(date_millis)
}

fn date_millis(value: &JsonValue) -> Option<i64> {
    match value {
        JsonValue::String(value) => parse_rfc3339_millis(value),
        JsonValue::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_f64().map(|value| value.round() as i64)),
        _ => None,
    }
}

fn parse_rfc3339_millis(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|dt| dt.timestamp_millis())
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
                .ok()
                .map(|dt| dt.and_utc().timestamp_millis())
        })
}

fn aidoku_source_for_manga(manga: &Manga, matcher: &Matcher) -> Result<SourceData> {
    if manga.source.reader == ReaderId::Aidoku {
        aidoku_source_id(&manga.source, &manga.title)?;
        return Ok(manga.source.clone());
    }

    matcher
        .find_matches(&manga.source)
        .map_err(|err| IRError::Matcher(err.to_string()))?
        .into_iter()
        .filter(|source| source.reader == ReaderId::Aidoku)
        .find(|source| source.external_id.is_some())
        .ok_or_else(|| {
            IRError::Matcher(format!(
                "couldn't find Aidoku source for {} ({:?})",
                manga.title, manga.source
            ))
        })
}

fn aidoku_source_id(source: &SourceData, manga_title: &str) -> Result<String> {
    source.external_id.clone().ok_or_else(|| {
        IRError::Matcher(format!(
            "missing Aidoku source id for {manga_title} ({source:?})"
        ))
    })
}

fn library_entry_to_plist(manga: &Manga, source_id: &str) -> PlistValue {
    let mut entry = Dictionary::new();
    let date_added = library_date_added(manga);
    let last_read = manga.history.iter().map(|history| history.last_read).max();
    let last_updated_chapters = manga
        .chapters
        .iter()
        .map(|chapter| chapter.date_uploaded)
        .filter(|date| *date != 0)
        .max();

    entry.insert(
        "dateAdded".into(),
        PlistValue::Date(plist_date_from_millis(date_added)),
    );
    entry.insert(
        "lastOpened".into(),
        PlistValue::Date(plist_date_from_millis(last_read.unwrap_or(date_added))),
    );
    entry.insert(
        "lastUpdated".into(),
        PlistValue::Date(plist_date_from_millis(
            last_updated_chapters.unwrap_or(date_added),
        )),
    );
    if let Some(last_read) = last_read {
        entry.insert(
            "lastRead".into(),
            PlistValue::Date(plist_date_from_millis(last_read)),
        );
    }
    if let Some(last_updated_chapters) = last_updated_chapters {
        entry.insert(
            "lastUpdatedChapters".into(),
            PlistValue::Date(plist_date_from_millis(last_updated_chapters)),
        );
    }
    entry.insert(
        "categories".into(),
        PlistValue::Array(
            manga
                .categories
                .iter()
                .cloned()
                .map(PlistValue::String)
                .collect(),
        ),
    );
    entry.insert("mangaId".into(), PlistValue::String(manga.id.clone()));
    entry.insert("sourceId".into(), PlistValue::String(source_id.to_owned()));

    PlistValue::Dictionary(entry)
}

fn library_date_added(manga: &Manga) -> i64 {
    manga.date_added.unwrap_or_else(|| {
        manga
            .history
            .iter()
            .map(|history| history.last_read)
            .chain(manga.chapters.iter().map(|chapter| chapter.date_uploaded))
            .max()
            .unwrap_or_default()
    })
}

fn manga_to_plist(manga: &Manga, source_id: &str) -> PlistValue {
    let mut item = Dictionary::new();
    item.insert("id".into(), PlistValue::String(manga.id.clone()));
    item.insert("sourceId".into(), PlistValue::String(source_id.to_owned()));
    item.insert("title".into(), PlistValue::String(manga.title.clone()));
    insert_optional_string(&mut item, "author", manga.author.as_ref());
    insert_optional_string(&mut item, "artist", manga.artist.as_ref());
    insert_optional_string(&mut item, "desc", manga.desc.as_ref());
    item.insert(
        "tags".into(),
        PlistValue::Array(manga.tags.iter().cloned().map(PlistValue::String).collect()),
    );
    insert_optional_string(&mut item, "cover", manga.cover_url.as_ref());
    if manga.id.starts_with("http://") || manga.id.starts_with("https://") {
        item.insert("url".into(), PlistValue::String(manga.id.clone()));
    }
    item.insert(
        "status".into(),
        PlistValue::Integer(manga.status.code().into()),
    );
    item.insert(
        "nsfw".into(),
        PlistValue::Integer(i64::from(manga.nsfw).into()),
    );
    item.insert("viewer".into(), PlistValue::Integer(0.into()));
    item.insert("neverUpdate".into(), PlistValue::Boolean(false));
    item.insert("chapterFlags".into(), PlistValue::Integer(0.into()));
    item.insert("editedKeys".into(), PlistValue::Integer(0.into()));

    PlistValue::Dictionary(item)
}

fn chapter_to_plist(
    chapter: &Chapter,
    manga: &Manga,
    source_id: &str,
    source_order: usize,
) -> PlistValue {
    let mut item = Dictionary::new();
    item.insert("sourceId".into(), PlistValue::String(source_id.to_owned()));
    item.insert("mangaId".into(), PlistValue::String(manga.id.clone()));
    item.insert("id".into(), PlistValue::String(chapter.id.clone()));
    item.insert("title".into(), PlistValue::String(chapter.title.clone()));
    if !chapter.scanlator.is_empty() {
        item.insert(
            "scanlator".into(),
            PlistValue::String(chapter.scanlator.clone()),
        );
    }
    if chapter.id.starts_with("http://") || chapter.id.starts_with("https://") {
        item.insert("url".into(), PlistValue::String(chapter.id.clone()));
    }
    item.insert(
        "lang".into(),
        PlistValue::String(match source_id.split('.').next() {
            Some(code) if !code.is_empty() && code != "multi" => code.to_owned(),
            _ => String::new(),
        }),
    );
    item.insert("chapter".into(), PlistValue::Real(chapter.chapter_number));
    if chapter.date_uploaded != 0 {
        item.insert(
            "dateUploaded".into(),
            PlistValue::Date(plist_date_from_millis(chapter.date_uploaded)),
        );
    }
    item.insert("locked".into(), PlistValue::Boolean(false));
    item.insert(
        "sourceOrder".into(),
        PlistValue::Integer((source_order as i64).into()),
    );

    PlistValue::Dictionary(item)
}

fn history_to_plist(
    history: &History,
    manga: &Manga,
    source_id: &str,
    chapter: Option<&Chapter>,
) -> PlistValue {
    let mut item = Dictionary::new();
    item.insert(
        "dateRead".into(),
        PlistValue::Date(plist_date_from_millis(history.last_read)),
    );
    item.insert("sourceId".into(), PlistValue::String(source_id.to_owned()));
    item.insert("chapterId".into(), PlistValue::String(history.url.clone()));
    item.insert("mangaId".into(), PlistValue::String(manga.id.clone()));
    item.insert(
        "progress".into(),
        PlistValue::Integer(
            chapter
                .map(|chapter| i64::from(chapter.last_page_read))
                .unwrap_or_default()
                .into(),
        ),
    );
    item.insert(
        "total".into(),
        PlistValue::Integer(
            chapter
                .and_then(|chapter| chapter.read.then_some(i64::from(chapter.last_page_read)))
                .unwrap_or_default()
                .into(),
        ),
    );
    item.insert(
        "completed".into(),
        PlistValue::Boolean(chapter.map(|chapter| chapter.read).unwrap_or_default()),
    );

    PlistValue::Dictionary(item)
}

fn synthesized_history_time(manga: &Manga, chapter: &Chapter) -> i64 {
    [
        chapter.last_modified,
        chapter.date_fetched,
        chapter.date_uploaded,
        manga.date_added.unwrap_or_default(),
    ]
        .into_iter()
        .max()
        .unwrap_or_default()
}

fn categories_to_plist(backup: &Backup) -> Vec<PlistValue> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();

    for category in &backup.categories {
        if seen.insert(category.name.clone()) {
            names.push(category.name.clone());
        }
    }

    for manga in &backup.manga {
        for category in &manga.categories {
            if seen.insert(category.clone()) {
                names.push(category.clone());
            }
        }
    }

    names
        .into_iter()
        .enumerate()
        .map(|(idx, name)| {
            let mut item = Dictionary::new();
            item.insert("title".into(), PlistValue::String(name));
            item.insert("sort".into(), PlistValue::Integer((idx as i64).into()));
            item.insert("group".into(), PlistValue::Boolean(false));
            PlistValue::Dictionary(item)
        })
        .collect()
}

fn insert_optional_string(dictionary: &mut Dictionary, key: &str, value: Option<&String>) {
    if let Some(value) = value {
        dictionary.insert(key.into(), PlistValue::String(value.clone()));
    }
}

fn plist_date_from_millis(millis: i64) -> plist::Date {
    let dt = DateTime::from_timestamp_millis(millis)
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());
    plist::Date::from(SystemTime::from(dt))
}