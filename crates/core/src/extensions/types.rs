use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::ReaderID;
use crate::extensions::ExtResult;
use crate::extensions::lang::Language;

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct Extension {
    pub reader: ReaderID,
    pub name: String,
    pub external_id: String,
    pub url: String,
    pub lang: Language,
    pub repo: ExtensionRepositoryInfo,
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct ExtensionRepositoryInfo {
    pub reader: ReaderID,
    pub name: Option<String>,
    pub short_name: Option<String>,
    pub website: Option<String>,
    pub signing_key: Option<String>,
}

/// A partially specified extension to search for. Everything but the app is
/// optional; an empty query matches whatever the repository has.
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct ExtensionQuery {
    pub reader: ReaderID,
    pub name: Option<String>,
    pub external_id: Option<String>,
    pub lang: Option<Language>,
    pub url: Option<String>,
}

impl ExtensionQuery {
    pub fn new(reader: ReaderID) -> Self {
        Self { reader, name: None, external_id: None, lang: None, url: None }
    }
    
    pub fn equivalent_of(extension: &Extension, reader: ReaderID) -> Self {
        Self {
            reader,
            name: Some(extension.name.clone()),
            external_id: Some(extension.external_id.clone()),
            lang: Some(extension.lang.clone()),
            url: Some(extension.url.clone())
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ExtensionMatch {
    pub extension: Arc<Extension>,
    /// `0.0..=1.0`, where 1.0 means the ids matched outright.
    pub score: f32,
}

/// Boxed by hand because `async fn` in a trait isn't dyn-safe.
pub type RefreshFuture<'a> = Pin<Box<dyn Future<Output = ExtResult<()>> + Send + 'a>>;

pub trait ExtensionRepository {
    fn info(&self) -> &ExtensionRepositoryInfo;

    /// Searches the index built by the last [`refresh`](Self::refresh); never
    /// goes out to the network itself.
    fn search(&self, query: &ExtensionQuery) -> ExtResult<Vec<ExtensionMatch>>;

    fn refresh(&mut self) -> RefreshFuture<'_> {
        Box::pin(async { Ok(()) })
    }
}
