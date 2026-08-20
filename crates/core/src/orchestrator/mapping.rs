use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use thiserror::Error;

use crate::ReaderID;
use crate::extensions::types::{Extension, ExtensionRepositoryInfo};

#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum MappingError {
    #[error(
        "extension '{extension}' belongs to {found}, but this mapping targets \
         {target}; the user has no way to install it in the target app"
    )]
    ForeignExtension {
        extension: String,
        target: ReaderID,
        found: ReaderID,
    },
}

/// A repository that could not be searched while this mapping was built.
///
/// Its candidates are missing from every extension here, so a source that came
/// out unresolved may only be unindexed. Carries the error rendered rather than
/// the error itself, so a mapping stays comparable and cloneable.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SearchFailure {
    pub repo: ExtensionRepositoryInfo,
    pub error: String,
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum UnresolvedReason {
    /// Nothing serving the target app turned anything up.
    NoCandidates,
    /// There were candidates, the policy just didn't like any of them.
    Declined { candidates: usize },
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct UnresolvedExtension {
    pub extension: Arc<Extension>,
    pub reason: UnresolvedReason,
}

/// A backup's extension table, resolved against a target app.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ExtensionMapping {
    target: ReaderID,
    resolved: HashMap<Arc<Extension>, Arc<Extension>>,
    unresolved: Vec<UnresolvedExtension>,
    /// The `Arc<Extension>` keys in `unresolved`, for a cheap membership check.
    unresolved_keys: HashSet<Arc<Extension>>,
    search_failures: Vec<SearchFailure>,
}

impl ExtensionMapping {
    pub fn new(target: ReaderID) -> Self {
        Self {
            target,
            resolved: HashMap::new(),
            unresolved: Vec::new(),
            unresolved_keys: HashSet::new(),
            search_failures: Vec::new(),
        }
    }

    pub const fn target(&self) -> ReaderID {
        self.target
    }

    /// Fallible because a mapping is also built by hand: a UI letting the user
    /// pick a target has no `resolve` filtering its candidates for it, and an
    /// extension belonging to another app would be written out as a source the
    /// user cannot install.
    pub fn insert(
        &mut self,
        source: Arc<Extension>,
        target: Arc<Extension>,
    ) -> Result<(), MappingError> {
        if target.reader != self.target {
            return Err(MappingError::ForeignExtension {
                extension: target.name.clone(),
                target: self.target,
                found: target.reader,
            });
        }

        // An extension the user manually mapped is no longer unresolved.
        let source = self.remove_unresolved(source);
        self.resolved.insert(source, target);
        Ok(())
    }

    pub fn mark_unresolved(&mut self, extension: Arc<Extension>, reason: UnresolvedReason) {
        self.unresolved_keys.insert(Arc::clone(&extension));
        self.unresolved
            .push(UnresolvedExtension { extension, reason });
    }

    pub fn get(&self, source: &Arc<Extension>) -> Option<&Arc<Extension>> {
        self.resolved.get(source)
    }

    /// Whether resolution looked at `source` at all, mapped or not. An entry
    /// whose extension isn't in here was never in `Backup::extensions` either.
    pub fn contains(&self, source: &Arc<Extension>) -> bool {
        self.resolved.contains_key(source) || self.unresolved_keys.contains(source)
    }

    pub fn resolved(&self) -> impl Iterator<Item = (&Arc<Extension>, &Arc<Extension>)> {
        self.resolved.iter()
    }

    pub fn unresolved(&self) -> &[UnresolvedExtension] {
        &self.unresolved
    }

    /// Recorded once per repo however many extensions hit it, since the failure
    /// is the repo's and not any one extension's.
    pub fn mark_repo_failed(&mut self, repo: ExtensionRepositoryInfo, error: &impl ToString) {
        if self.search_failures.iter().any(|f| f.repo == repo) {
            return;
        }
        self.search_failures.push(SearchFailure {
            repo,
            error: error.to_string(),
        });
    }

    /// Repositories that could not be searched. Non-empty means `unresolved`
    /// overstates what the target app is really missing.
    pub fn search_failures(&self) -> &[SearchFailure] {
        &self.search_failures
    }

    pub const fn is_fully_resolved(&self) -> bool {
        self.unresolved.is_empty()
    }

    /// Re-classify `source` as no longer unresolved, returning the original
    /// `Arc` so the caller chains it into resolution or a manual mapping.
    fn remove_unresolved(&mut self, source: Arc<Extension>) -> Arc<Extension> {
        if let Some(pos) = self.unresolved.iter().position(|u| u.extension == source) {
            let removed = self.unresolved.remove(pos);
            self.unresolved_keys.remove(&removed.extension);
            removed.extension
        } else {
            source
        }
    }
}
