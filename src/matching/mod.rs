use std::fmt;

pub mod aidoku;
pub mod tachiyomi;

#[derive(thiserror::Error, Debug)]
pub enum MatcherError {
    #[error("Source error {0}: {1}")]
    SourceError (SourceId, String),

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
    Tachiyomi
}
impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceId::Aidoku => write!(f, "Aidoku"),
            SourceId::Tachiyomi => write!(f, "Keiyoushi"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SourceData {
    pub source: SourceId,
    pub name: Option<String>,
    pub external_id: Option<String>,
    pub url: Option<String>,
}

pub trait Source {
    fn id(&self) -> SourceId;

    fn search(&self, query: &SourceData) -> Result<Vec<SourceData>>;
}

pub struct Matcher {
    sources: Vec<Box<dyn Source>>,
}

impl Matcher {
    pub fn new() -> Self {
        Self { sources: Vec::new() }
    }

    pub fn register_source(&mut self, source: Box<dyn Source>) {
        self.sources.push(source);
    }

    pub fn find_matches(&self, query: &SourceData) -> Result<Vec<SourceData>> {
        let mut all_results = Vec::new();

        for source in &self.sources {
            if query.source == source.id() { continue; }
            let r = source.search(query)?;
            all_results.extend(r);
        }

        Ok(all_results)
    }
}