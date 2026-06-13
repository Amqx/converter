// Sources:
// 1: Keiyoushi's official repo (https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json)

use super::SourceId::Tachiyomi;
use super::{Language, Result, Source, SourceData, SourceId, normalized_key};
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
    fn to_source_data(&self) -> SourceData {
        SourceData {
            source: Tachiyomi,
            name: Some(self.name.clone()),
            external_id: Some(self.id.to_string()),
            url: Some(self.url.clone()),
            lang: Some(self.lang),
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

pub struct Keiyoushi {
    entries: Vec<TachiyomiEntry>,

    by_name: HashMap<String, Vec<usize>>,
    by_url: HashMap<String, Vec<usize>>,
    by_id: HashMap<i64, usize>,
}

impl Keiyoushi {
    const ID: SourceId = Tachiyomi;
    const INDEX_URL: &str =
        "https://raw.githubusercontent.com/keiyoushi/extensions/repo/index.min.json";

    pub fn new() -> Result<Self> {
        let mut s = Self::empty();
        s.get()?;
        Ok(s)
    }

    fn empty() -> Self {
        Self {
            entries: Vec::new(),
            by_name: HashMap::new(),
            by_url: HashMap::new(),
            by_id: HashMap::new(),
        }
    }

    fn get(&mut self) -> Result<()> {
        let client = Client::builder()
            .user_agent("Tachiyomi-Source-Fetcher")
            .build()?;

        let sources: Vec<TachiyomiIndex> = client
            .get(Self::INDEX_URL)
            .send()?
            .error_for_status()?
            .json()?;

        self.load_indexes(sources)
    }

    fn load_indexes(&mut self, sources: Vec<TachiyomiIndex>) -> Result<()> {
        self.entries.clear();
        self.by_name.clear();
        self.by_url.clear();
        self.by_id.clear();

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
    ) {
        for idx in indexes {
            Self::push_entry(results, seen_ids, &entries[*idx]);
        }
    }

    fn push_entry(
        results: &mut Vec<SourceData>,
        seen_ids: &mut HashSet<i64>,
        entry: &TachiyomiEntry,
    ) {
        if !seen_ids.insert(entry.id) {
            return;
        }

        results.push(entry.to_source_data());
    }
}

impl Source for Keiyoushi {
    fn id(&self) -> SourceId {
        Self::ID
    }

    fn search(&self, query: &SourceData) -> Result<Vec<SourceData>> {
        let mut results = Vec::new();
        let mut seen_ids = HashSet::new();

        if let Some(url) = &query.url
            && let Some(indexes) = self.by_url.get(&normalized_key(url))
        {
            Self::push_entry_indexes(&mut results, &mut seen_ids, &self.entries, indexes);
        }

        if let Some(external_id) = &query.external_id
            && let Ok(id) = external_id.parse::<i64>()
            && let Some(idx) = self.by_id.get(&id)
        {
            let entry = &self.entries[*idx];
            Self::push_entry(&mut results, &mut seen_ids, entry);
        }

        if let Some(name) = &query.name
            && let Some(indexes) = self.by_name.get(&normalized_key(name))
        {
            Self::push_entry_indexes(&mut results, &mut seen_ids, &self.entries, indexes);
        }

        Ok(results)
    }
}
