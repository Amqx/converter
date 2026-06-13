use std::fmt;

pub mod aidoku;
pub mod tachiyomi;

#[derive(thiserror::Error, Debug)]
pub enum MatcherError {
    #[error("Source error {0}: {1}")]
    SourceError(SourceId, String),

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, PartialEq, Eq)]
pub struct SourceData {
    pub source: SourceId,
    pub name: Option<String>,
    pub external_id: Option<String>,
    pub url: Option<String>,
    pub lang: Option<Language>,
}

impl SourceData {
    pub fn new(source: SourceId) -> Self {
        Self {
            source,
            name: None,
            external_id: None,
            url: None,
            lang: None,
        }
    }
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

    pub fn list_sources(&self) -> Vec<SourceId> {
        self.sources.iter().map(|s| s.id()).collect()
    }

    pub fn find_matches(&self, query: &SourceData) -> Result<Vec<SourceData>> {
        let mut all_results = Vec::new();

        for source in &self.sources {
            if query.source == source.id() {
                continue;
            }
            all_results.extend(source.search(query)?);
        }

        Ok(all_results)
    }
}
