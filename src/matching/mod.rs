use clap::ValueEnum;
use std::fmt;
use tracing::{debug, trace};

pub mod aidoku;
pub mod tachiyomi;

#[derive(thiserror::Error, Debug)]
pub enum MatcherError {
    #[error("Network error")]
    Network(#[from] reqwest::Error),

    #[error("File error")]
    File(#[from] std::io::Error),

    #[error("Json error")]
    Json(#[from] serde_json::Error),

    #[error("Parse int error")]
    ParseInt(#[from] std::num::ParseIntError),
}
pub type Result<T> = std::result::Result<T, MatcherError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ValueEnum)]
pub enum SourceId {
    Aidoku,
    Tachiyomi,
}
impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceId::Aidoku => write!(f, "Aidoku"),
            SourceId::Tachiyomi => write!(f, "Keiyoushi"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Iso(isolang::Language),
    All,
}

impl Language {
    pub fn from_code(lang: &str) -> Self {
        if lang.eq_ignore_ascii_case("all") {
            Language::All
        } else {
            let normalized = lang.split('-').next().unwrap_or(lang);
            match isolang::Language::from_locale(normalized)
                .or_else(|| isolang::Language::from_639_1(normalized))
            {
                Some(iso_lang) => Language::Iso(iso_lang),
                None => Language::All, // Fallback if unknown code points are found
            }
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Language::Iso(l) => write!(f, "{}", l.to_name()),
            Language::All => write!(f, "Multilingual (all)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRepo {
    pub base_url: Option<String>,
    pub name: Option<String>,
    pub short_name: Option<String>,
    pub website: Option<String>,
    pub signing_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceData {
    pub source: SourceId,
    pub name: Option<String>,
    pub external_id: Option<String>,
    pub url: Option<String>,
    pub lang: Option<Language>,
    pub repo: Option<SourceRepo>,
}

pub(super) fn normalized_key(value: &str) -> String {
    value.to_lowercase()
}

pub trait Source {
    fn id(&self) -> SourceId;

    fn search(&self, query: &SourceData) -> Result<Vec<SourceData>>;
}

pub struct Matcher {
    sources: Vec<Box<dyn Source>>,
}

impl Default for Matcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Matcher {
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    pub fn register_source(&mut self, source: Box<dyn Source>) {
        self.sources.push(source);
    }

    pub fn find_matches(&self, query: &SourceData) -> Result<Vec<SourceData>> {
        let mut all_results = Vec::new();

        debug!(
            query_source = ?query.source,
            query_name = ?query.name,
            query_external_id = ?query.external_id,
            query_url = ?query.url,
            registered_sources = self.sources.len(),
            "Searching for matching sources"
        );

        for source in &self.sources {
            if query.source == source.id() {
                trace!(
                    source = ?source.id(),
                    "Skipping matcher source because it matches the query source"
                );
                continue;
            }
            trace!(source = ?source.id(), "Searching matcher source");
            let results = source.search(query)?;
            debug!(
                source = ?source.id(),
                matches = results.len(),
                "Matcher source returned results"
            );
            trace!(source = ?source.id(), ?results, "Matcher source result details");
            all_results.extend(results);
        }

        debug!(matches = all_results.len(), "Finished source matching");
        Ok(all_results)
    }
}
