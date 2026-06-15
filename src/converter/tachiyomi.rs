use super::{
    Backup, BackupReader, BackupWriter, Category, Chapter, History, IRError, Manga, MangaStatus,
    Result,
};
use crate::matching::{Matcher, SourceData, SourceId, SourceRepo};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use prost::Message;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use tracing::{debug, info, trace, warn};

pub struct TachiyomiConverter;

#[allow(dead_code)]
mod proto {
    include!(concat!(env!("OUT_DIR"), "/tachiyomisy.backup.rs"));
}

const BLACKLISTED_SOURCE_IDS: &[i64] = &[
    0,    // Local sources
    6969, // Merged sources
    6902, // Ex-h
];

#[derive(Debug)]
struct TachiyomiSource {
    id: i64,
    name: Option<String>,
    repo: Option<SourceRepo>,
}

impl BackupReader for TachiyomiConverter {
    fn read_backup(bytes: &[u8]) -> Result<Backup> {
        let bytes = decode(bytes)?;
        let backup = proto::Backup::decode(bytes.as_slice())?;
        let source_names = source_names_by_id(&backup.backup_sources);
        let category_lookup = category_names_by_id(&backup.backup_categories);

        let categories = backup
            .backup_categories
            .iter()
            .enumerate()
            .map(category_to_ir)
            .collect();

        let manga = backup
            .backup_manga
            .into_iter()
            .filter(|m| {
                if BLACKLISTED_SOURCE_IDS.contains(&m.source) {
                    warn!(
                        source_id = m.source,
                        manga_url = %m.url,
                        title = m.title.as_deref().unwrap_or_default(),
                        "skipping Tachiyomi manga from blacklisted source"
                    );
                    false
                } else {
                    true
                }
            })
            .map(|m| manga_to_ir(m, &source_names, &category_lookup))
            .collect();

        Ok(Backup {
            manga,
            categories
        })
    }
}

impl BackupWriter for TachiyomiConverter {
    fn write_backup(backup: &Backup, matcher: &Matcher) -> Result<Vec<u8>> {
        debug!(
            manga = backup.manga.len(),
            categories = backup.categories.len(),
            "Building Tachiyomi backup"
        );

        let mut source_ids = HashSet::new();
        let mut backup_sources = Vec::new();
        let mut repo_base_urls = HashSet::new();
        let mut backup_extension_repo = Vec::new();
        let category_ids = category_ids_by_name(&backup.categories);

        let mut failed_sources = HashSet::new();
        let mut backup_manga = Vec::new();
        let mut skipped_manga = 0usize;
        for manga in &backup.manga {
            trace!(
                title = %manga.title,
                chapters = manga.chapters.len(),
                history_entries = manga.history.len(),
                "Converting manga to Tachiyomi"
            );
            let source_key = (
                manga.source.source,
                manga.source.external_id.clone(),
                manga.source.name.clone(),
            );
            if failed_sources.contains(&source_key) {
                skipped_manga += 1;
                warn!(
                    title = %manga.title,
                    source = source_key.2,
                    "skipping manga because its source previously failed to match"
                );
                continue;
            }
            let tachiyomi_source = match tachiyomi_source_for_manga(manga, matcher) {
                Ok(source) => source,
                Err(err) => {
                    skipped_manga += 1;
                    warn!(
                        title = %manga.title,
                        source = source_key.2,
                        error = %err,
                        "failed to match source; skipping this and subsequent manga from the same source"
                    );
                    failed_sources.insert(source_key);
                    continue;
                }
            };

            if source_ids.insert(tachiyomi_source.id) {
                backup_sources.push(proto::BackupSource {
                    name: tachiyomi_source.name.clone(),
                    source_id: tachiyomi_source.id,
                });
            }
            if let Some(repo) = tachiyomi_source.repo.as_ref()
                && let Some(base_url) = repo.base_url.as_ref()
                && repo_base_urls.insert(base_url.clone())
            {
                backup_extension_repo.push(repo_to_proto(repo));
            }

            backup_manga.push(manga_to_proto(manga, tachiyomi_source.id, &category_ids));
        }

        let backup_categories: Vec<_> = backup
            .categories
            .iter()
            .enumerate()
            .map(category_to_proto)
            .collect();
        info!(
            converted_manga = backup_manga.len(),
            skipped_manga,
            sources = backup_sources.len(),
            extension_repos = backup_extension_repo.len(),
            "Finished building Tachiyomi backup data"
        );

        let proto_backup = proto::Backup {
            backup_manga,
            backup_categories,
            backup_sources,
            backup_preferences: Vec::new(),
            backup_source_preferences: Vec::new(),
            backup_extension_repo,
            backup_saved_searches: Vec::new(),
        };

        let bytes = encode(&proto_backup)?;
        debug!(bytes = bytes.len(), "Finished serializing Tachiyomi backup");
        Ok(bytes)
    }
}

fn decode(bytes: &[u8]) -> Result<Vec<u8>> {
    const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
    if bytes.starts_with(&GZIP_MAGIC) {
        let mut decoder = GzDecoder::new(bytes);
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded)?;
        Ok(decoded)
    } else {
        Ok(bytes.to_vec())
    }
}

fn encode(backup: &proto::Backup) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&backup.encode_to_vec())
        .map_err(|err| IRError::Serialize(format!("failed to compress backup: {err}")))?;
    encoder
        .finish()
        .map_err(|err| IRError::Serialize(format!("failed to finish compressed backup: {err}")))
}

fn source_names_by_id(sources: &[proto::BackupSource]) -> HashMap<i64, String> {
    sources
        .iter()
        .filter_map(|source| source.name.clone().map(|name| (source.source_id, name)))
        .collect()
}

fn category_names_by_id(categories: &[proto::BackupCategory]) -> HashMap<i64, String> {
    let mut names = HashMap::new();

    for (idx, category) in categories.iter().enumerate() {
        names
            .entry(category.order.unwrap_or(idx as i64))
            .or_insert_with(|| category.name.clone());
    }

    for category in categories {
        for id in [category.id, category.uid].into_iter().flatten() {
            names.entry(id).or_insert_with(|| category.name.clone());
        }
    }

    names
}

fn category_to_ir((idx, category): (usize, &proto::BackupCategory)) -> Category {
    Category {
        name: category.name.clone(),
        id: category_key(category, idx).to_string(),
        last_modified: category.last_modified_at.unwrap_or_default(),
    }
}

fn category_key(category: &proto::BackupCategory, idx: usize) -> i64 {
    category
        .id
        .or(category.uid)
        .or(category.order)
        .unwrap_or(idx as i64)
}

fn manga_to_ir(
    manga: proto::BackupManga,
    source_names: &HashMap<i64, String>,
    category_lookup: &HashMap<i64, String>,
) -> Manga {
    Manga {
        id: manga.url.clone(),
        source: SourceData {
            source: SourceId::Tachiyomi,
            name: source_names.get(&manga.source).cloned(),
            external_id: Some(manga.source.to_string()),
            url: None,
            lang: None,
            repo: None,
        },
        title: manga
            .custom_title
            .or(manga.title)
            .unwrap_or_else(|| manga.url.clone()),
        author: manga.custom_author.or(manga.author),
        artist: manga.custom_artist.or(manga.artist),
        desc: manga.custom_description.or(manga.description),
        tags: if manga.custom_genre.is_empty() {
            manga.genre
        } else {
            manga.custom_genre
        },
        cover_url: manga.custom_thumbnail_url.or(manga.thumbnail_url),
        status: status_to_ir(manga.custom_status.or(manga.status).unwrap_or_default()),
        nsfw: false,
        date_added: manga.date_added,
        categories: manga
            .categories
            .iter()
            .filter_map(|category_id| category_lookup.get(category_id).cloned())
            .collect(),
        chapters: manga.chapters.into_iter().map(chapter_to_ir).collect(),
        history: manga.history.into_iter().map(history_to_ir).collect(),

        // Tachiyomi has this absurd behaviour where if the series is in library, they completely
        // omit the favorite field, and only include it if it's not in library (false)
        // Easy fix but holy f*** is this stupid behaviour
        favorite: manga.favorite.unwrap_or(true),
    }
}

fn chapter_to_ir(chapter: proto::BackupChapter) -> Chapter {
    Chapter {
        id: chapter.url,
        title: chapter.name,
        scanlator: chapter.scanlator.unwrap_or_default(),
        date_fetched: chapter.date_fetch.unwrap_or_default(),
        date_uploaded: chapter.date_upload.unwrap_or_default(),
        chapter_number: f64::from(chapter.chapter_number.unwrap_or_default()),
        last_modified: chapter.last_modified_at.unwrap_or_default(),
        read: chapter.read.unwrap_or_default(),
        last_page_read: chapter
            .last_page_read
            .unwrap_or_default()
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
    }
}

fn history_to_ir(history: proto::BackupHistory) -> History {
    History {
        url: history.url,
        last_read: history.last_read,
    }
}

fn status_to_ir(status: i32) -> MangaStatus {
    match status {
        1 => MangaStatus::Ongoing,
        2 => MangaStatus::Completed,
        3 => MangaStatus::Licensed,
        4 => MangaStatus::PublishingFinished,
        5 => MangaStatus::Cancelled,
        6 => MangaStatus::OnHiatus,
        _ => MangaStatus::Unknown,
    }
}

fn category_ids_by_name(categories: &[Category]) -> HashMap<String, i64> {
    categories
        .iter()
        .enumerate()
        .map(|(idx, category)| (category.name.clone(), idx as i64))
        .collect()
}

fn category_to_proto((idx, category): (usize, &Category)) -> proto::BackupCategory {
    proto::BackupCategory {
        name: category.name.clone(),
        order: Some(idx as i64),
        id: Some(category.id.parse::<i64>().unwrap_or(idx as i64)),
        flags: None,
        version: None,
        uid: None,
        last_modified_at: Some(category.last_modified),
    }
}

fn tachiyomi_source_for_manga(manga: &Manga, matcher: &Matcher) -> Result<TachiyomiSource> {
    if manga.source.source == SourceId::Tachiyomi {
        return Ok(TachiyomiSource {
            id: parse_source_id(&manga.source, &manga.title)?,
            name: manga.source.name.clone(),
            repo: manga.source.repo.clone(),
        });
    }

    matcher
        .find_matches(&manga.source)
        .map_err(|err| IRError::Matcher(err.to_string()))?
        .into_iter()
        .filter(|source| source.source == SourceId::Tachiyomi)
        .find_map(|source| {
            let id = source.external_id.as_deref()?.parse::<i64>().ok()?;
            Some(TachiyomiSource {
                id,
                name: source.name,
                repo: source.repo,
            })
        })
        .ok_or_else(|| {
            IRError::Matcher(format!(
                "couldn't find Tachiyomi source for {} ({:?})",
                manga.title, manga.source
            ))
        })
}

fn parse_source_id(source: &SourceData, manga_title: &str) -> Result<i64> {
    source
        .external_id
        .as_deref()
        .ok_or_else(|| {
            IRError::Matcher(format!(
                "missing Tachiyomi source id for {manga_title} ({source:?})"
            ))
        })?
        .parse::<i64>()
        .map_err(|err| {
            IRError::Matcher(format!(
                "invalid Tachiyomi source id for {manga_title} ({source:?}): {err}"
            ))
        })
}

fn repo_to_proto(repo: &SourceRepo) -> proto::BackupExtensionRepos {
    proto::BackupExtensionRepos {
        base_url: repo.base_url.clone().unwrap_or_default(),
        name: repo.name.clone().unwrap_or_default(),
        short_name: repo.short_name.clone(),
        website: repo.website.clone().unwrap_or_default(),
        signing_key_fingerprint: repo.signing_key.clone().unwrap_or_default(),
    }
}

fn manga_to_proto(
    manga: &Manga,
    tachiyomi_source_id: i64,
    category_ids: &HashMap<String, i64>,
) -> proto::BackupManga {
    proto::BackupManga {
        source: tachiyomi_source_id,
        url: manga.id.clone(),
        title: Some(manga.title.clone()),
        artist: manga.artist.clone(),
        author: manga.author.clone(),
        description: manga.desc.clone(),
        genre: manga.tags.clone(),
        status: Some(status_to_proto(&manga.status)),
        thumbnail_url: manga.cover_url.clone(),
        date_added: manga.date_added,
        viewer: None,
        chapters: manga.chapters.iter().map(chapter_to_proto).collect(),
        categories: manga
            .categories
            .iter()
            .filter_map(|category| {
                category_ids
                    .get(category)
                    .copied()
                    .or_else(|| category.parse::<i64>().ok())
            })
            .collect(),
        tracking: Vec::new(),
        favorite: Some(manga.favorite),
        chapter_flags: None,
        viewer_flags: None,
        history: manga.history.iter().map(history_to_proto).collect(),
        update_strategy: None,
        last_modified_at: None,
        favorite_modified_at: None,
        excluded_scanlators: Vec::new(),
        version: None,
        notes: None,
        initialized: None,
        merged_manga_references: Vec::new(),
        flat_metadata: None,
        custom_status: None,
        custom_thumbnail_url: None,
        custom_title: None,
        custom_artist: None,
        custom_author: None,
        custom_description: None,
        custom_genre: Vec::new(),
    }
}

fn chapter_to_proto(chapter: &Chapter) -> proto::BackupChapter {
    proto::BackupChapter {
        url: chapter.id.clone(),
        name: chapter.title.clone(),
        scanlator: (!chapter.scanlator.is_empty()).then(|| chapter.scanlator.clone()),
        read: Some(chapter.read),
        bookmark: None,
        last_page_read: Some(i64::from(chapter.last_page_read)),
        date_fetch: Some(chapter.date_fetched),
        date_upload: Some(chapter.date_uploaded),
        chapter_number: Some(chapter.chapter_number as f32),
        source_order: None,
        last_modified_at: Some(chapter.last_modified),
        version: None,
    }
}

fn history_to_proto(history: &History) -> proto::BackupHistory {
    proto::BackupHistory {
        url: history.url.clone(),
        last_read: history.last_read,
        read_duration: None,
    }
}

fn status_to_proto(status: &MangaStatus) -> i32 {
    match status {
        MangaStatus::Unknown => 0,
        MangaStatus::Ongoing => 1,
        MangaStatus::Completed => 2,
        MangaStatus::Licensed => 3,
        MangaStatus::PublishingFinished => 4,
        MangaStatus::Cancelled => 5,
        MangaStatus::OnHiatus => 6,
    }
}
