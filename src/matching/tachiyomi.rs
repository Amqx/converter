use super::{Language, Result, Source, SourceData, SourceRepo, normalized_key};
use crate::reader::ReaderId;
use crate::reader::ReaderId::Tachiyomi;
use crate::report::RepoInfo;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Debug, Clone)]
struct TachiyomiEntry {
    name: String,
    url: String,
    id: i64,
    lang: Language,
}
impl fmt::Display for TachiyomiEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}] ({}): {}",
            self.name, self.lang, self.id, self.url,
        )
    }
}

impl TachiyomiEntry {
    fn to_source_data(&self, repo: Option<SourceRepo>) -> SourceData {
        SourceData {
            reader: Tachiyomi,
            name: Some(self.name.clone()),
            external_id: Some(self.id.to_string()),
            url: Some(self.url.clone()),
            lang: Some(self.lang),
            repo,
        }
    }
}

#[derive(Deserialize, Debug)]
struct TachiyomiIndex {
    lang: Option<String>,
    sources: Vec<TachiyomiIndexSource>,
}

#[derive(Deserialize, Debug)]
struct TachiyomiIndexSource {
    name: String,
    id: String,
    lang: Option<String>,
    #[serde(rename = "baseUrl")]
    base_url: String,
}

#[derive(Deserialize, Debug)]
struct TachiyomiRepoJson {
    meta: TachiyomiRepoMeta,
}

#[derive(Deserialize, Debug)]
struct TachiyomiRepoMeta {
    name: String,
    website: String,
    #[serde(rename = "signingKeyFingerprint")]
    signing_key: String,
}

#[derive(Clone, Debug, Default)]
pub struct TachiyomiRepoInfo {
    base_url: String,
    name: String,
    short_name: String,
    website: String,
    signing_key: String,
}

impl TachiyomiRepoInfo {
    fn to_source_repo(&self) -> Option<SourceRepo> {
        (!self.base_url.is_empty()).then(|| SourceRepo {
            base_url: Some(self.base_url.clone()),
            name: Some(self.name.clone()),
            short_name: Some(self.short_name.clone()),
            website: Some(self.website.clone()),
            signing_key: Some(self.signing_key.clone()),
        })
    }
}

pub struct TachiyomiExtensionRepo {
    info: TachiyomiRepoInfo,
    index_url: String,
    entries: Vec<TachiyomiEntry>,

    by_name: HashMap<String, Vec<usize>>,
    by_url: HashMap<String, Vec<usize>>,
    by_id: HashMap<i64, usize>,
}

impl TachiyomiExtensionRepo {
    const READER_ID: ReaderId = Tachiyomi;

    pub fn new(index_url: impl Into<String>) -> Result<Self> {
        let mut s = Self::empty(index_url);
        s.get()?;
        Ok(s)
    }

    fn empty(index_url: impl Into<String>) -> Self {
        Self {
            info: TachiyomiRepoInfo::default(),
            index_url: index_url.into(),
            entries: Vec::new(),
            by_name: HashMap::new(),
            by_url: HashMap::new(),
            by_id: HashMap::new(),
        }
    }

    pub fn default() -> Result<Self> {
        pub const KEIYOUSHI_INDEX_URL: &str =
            "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json";

        TachiyomiExtensionRepo::new(KEIYOUSHI_INDEX_URL)
    }

    fn get(&mut self) -> Result<()> {
        let client = Client::builder()
            .user_agent("Tachiyomi-Source-Fetcher")
            .build()?;

        self.get_repo_info(&client)?;

        let sources: Vec<TachiyomiIndex> = client
            .get(&self.index_url)
            .send()?
            .error_for_status()?
            .json()?;

        self.load_indexes(sources)
    }

    fn repo_info_url(&self) -> String {
        self.index_url.rsplit_once('/').map_or_else(
            || "repo.json".to_string(),
            |(base, _)| format!("{base}/repo.json"),
        )
    }

    fn get_repo_info(&mut self, client: &Client) -> Result<()> {
        let repo: TachiyomiRepoJson = client
            .get(self.repo_info_url())
            .send()?
            .error_for_status()?
            .json()?;

        self.info = TachiyomiRepoInfo {
            base_url: self.index_url.clone(),
            short_name: normalized_key(&repo.meta.name),
            name: repo.meta.name,
            website: repo.meta.website,
            signing_key: repo.meta.signing_key,
        };

        Ok(())
    }

    fn load_indexes(&mut self, sources: Vec<TachiyomiIndex>) -> Result<()> {
        for extension in sources {
            let extension_lang = extension.lang.as_deref().map(Language::from_code);

            for source in extension.sources {
                let lang = source
                    .lang
                    .as_deref()
                    .map(Language::from_code)
                    .or(extension_lang)
                    .unwrap_or(Language::All);

                self.entries.push(TachiyomiEntry {
                    name: source.name,
                    url: source.base_url,
                    id: source.id.parse::<i64>()?,
                    lang,
                });
            }
        }

        for (idx, entry) in self.entries.iter().enumerate() {
            self.by_name
                .entry(normalized_key(&entry.name))
                .or_default()
                .push(idx);

            self.by_url
                .entry(normalized_key(&entry.url))
                .or_default()
                .push(idx);

            self.by_id.insert(entry.id, idx);
        }

        Ok(())
    }
    fn push_entry_indexes(
        results: &mut Vec<SourceData>,
        seen_ids: &mut HashSet<i64>,
        entries: &[TachiyomiEntry],
        indexes: &[usize],
        repo: Option<&SourceRepo>,
    ) {
        for idx in indexes {
            Self::push_entry(results, seen_ids, &entries[*idx], repo);
        }
    }

    fn push_entry(
        results: &mut Vec<SourceData>,
        seen_ids: &mut HashSet<i64>,
        entry: &TachiyomiEntry,
        repo: Option<&SourceRepo>,
    ) {
        if !seen_ids.insert(entry.id) {
            return;
        }

        results.push(entry.to_source_data(repo.cloned()));
    }
}

impl Source for TachiyomiExtensionRepo {
    fn reader_id(&self) -> ReaderId {
        Self::READER_ID
    }

    fn search(&self, query: &SourceData) -> Result<Vec<SourceData>> {
        let mut results = Vec::new();
        let mut seen_ids = HashSet::new();
        let repo = self.info.to_source_repo();

        if let Some(url) = &query.url
            && let Some(indexes) = self.by_url.get(&normalized_key(url))
        {
            Self::push_entry_indexes(
                &mut results,
                &mut seen_ids,
                &self.entries,
                indexes,
                repo.as_ref(),
            );
        }

        if let Some(external_id) = &query.external_id
            && let Ok(id) = external_id.parse::<i64>()
            && let Some(idx) = self.by_id.get(&id)
        {
            let entry = &self.entries[*idx];
            Self::push_entry(&mut results, &mut seen_ids, entry, repo.as_ref());
        }

        if let Some(name) = &query.name
            && let Some(indexes) = self.by_name.get(&normalized_key(name))
        {
            Self::push_entry_indexes(
                &mut results,
                &mut seen_ids,
                &self.entries,
                indexes,
                repo.as_ref(),
            );
        }

        Ok(results)
    }

    fn describe(&self) -> RepoInfo {
        RepoInfo {
            reader: Self::READER_ID,
            name: self.info.name.clone(),
            url: self.index_url.clone(),
        }
    }
}