use super::SourceId::Aidoku;
use super::{Language, Result, Source, SourceData, SourceId, normalized_key};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Debug, Clone)]
struct AidokuEntry {
    name: String,
    id: String,
    url: String,
    lang: Language,
}
impl fmt::Display for AidokuEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}] ({}): {}",
            self.name, self.lang, self.id, self.url
        )
    }
}

impl AidokuEntry {
    fn to_source_data(&self) -> SourceData {
        SourceData {
            source: Aidoku,
            name: Some(self.name.clone()),
            external_id: Some(self.id.clone()),
            url: Some(self.url.clone()),
            lang: Some(self.lang),
        }
    }
}

#[derive(Deserialize, Debug)]
struct AidokuIndex {
    sources: Vec<AidokuIndexSource>,
}

#[derive(Deserialize, Debug)]
struct AidokuIndexSource {
    id: String,
    name: String,
    #[serde(default)]
    languages: Vec<String>,

    #[serde(rename = "baseURL")]
    base_url: String,
}

pub struct AidokuSourceList {
    index_url: String,
    entries: Vec<AidokuEntry>,

    by_name: HashMap<String, Vec<usize>>,
    by_url: HashMap<String, Vec<usize>>,
    by_id: HashMap<String, Vec<usize>>,
}

impl AidokuSourceList {
    const ID: SourceId = Aidoku;

    pub fn new(index_url: impl Into<String>) -> Result<Self> {
        let mut s = Self::empty(index_url);
        s.get()?;
        Ok(s)
    }

    pub fn default() -> Result<Self> {
        const AMQX_SOURCE_LIST_URL: &str = "https://amqx.github.io/sources/index.min.json";
        Self::new(AMQX_SOURCE_LIST_URL)
    }

    fn empty(index_url: impl Into<String>) -> Self {
        Self {
            index_url: index_url.into(),
            entries: Vec::new(),
            by_name: HashMap::new(),
            by_url: HashMap::new(),
            by_id: HashMap::new(),
        }
    }

    fn get(&mut self) -> Result<()> {
        let client = Client::builder()
            .user_agent("Aidoku-Source-Fetcher")
            .build()?;

        let index: AidokuIndex = client
            .get(&self.index_url)
            .send()?
            .error_for_status()?
            .json()?;

        self.load_indexes(index);
        Ok(())
    }

    fn load_indexes(&mut self, index: AidokuIndex) {
        for source in index.sources {
            let languages = if source.languages.is_empty() {
                vec![Language::All]
            } else {
                source
                    .languages
                    .iter()
                    .map(|lang| Language::from_code(lang))
                    .collect()
            };

            for lang in languages {
                self.entries.push(AidokuEntry {
                    name: source.name.clone(),
                    id: source.id.clone(),
                    url: source.base_url.clone(),
                    lang,
                });
            }
        }

        for (idx, entry) in self.entries.iter().enumerate() {
            self.by_name
                .entry(normalized_key(&entry.name))
                .or_default()
                .push(idx);

            self.by_id
                .entry(normalized_key(&entry.id))
                .or_default()
                .push(idx);

            self.by_url
                .entry(normalized_key(&entry.url))
                .or_default()
                .push(idx);
        }
    }

    fn push_entry_indexes(
        results: &mut Vec<SourceData>,
        seen: &mut HashSet<usize>,
        entries: &[AidokuEntry],
        indexes: &[usize],
    ) {
        for idx in indexes {
            if seen.insert(*idx) {
                results.push(entries[*idx].to_source_data());
            }
        }
    }
}

impl Source for AidokuSourceList {
    fn id(&self) -> SourceId {
        Self::ID
    }

    fn search(&self, query: &SourceData) -> Result<Vec<SourceData>> {
        let mut matches = Vec::new();
        let mut seen = HashSet::new();

        if let Some(url) = &query.url
            && let Some(indexes) = self.by_url.get(&normalized_key(url))
        {
            Self::push_entry_indexes(&mut matches, &mut seen, &self.entries, indexes);
        }

        if let Some(external_id) = &query.external_id
            && let Some(indexes) = self.by_id.get(&normalized_key(external_id))
        {
            Self::push_entry_indexes(&mut matches, &mut seen, &self.entries, indexes);
        }

        if let Some(name) = &query.name
            && let Some(indexes) = self.by_name.get(&normalized_key(name))
        {
            Self::push_entry_indexes(&mut matches, &mut seen, &self.entries, indexes);
        }

        Ok(matches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_multiple_lists() {
        let amqx = AidokuSourceList::default().unwrap();
        let community = AidokuSourceList::new("https://aidoku-community.github.io/sources/index.min.json").unwrap();

        let query = SourceData{
            source: SourceId::Tachiyomi,
            name: Some("MangaDex".into()),
            external_id: Some("2499283573021220255".into()),
            url: Some("https://mangadex.org".into()),
            lang: Some(Language::All)
        };

        let amqx_response = amqx.search(&query).unwrap();
        let community_response = community.search(&query).unwrap();

        println!("Amqx result: {:?}", amqx_response);
        println!("Community result: {:?}", community_response);
    }
}