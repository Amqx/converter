use std::collections::HashMap;
use std::sync::Arc;

use crate::ReaderID;
use crate::extensions::types::Extension;

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum UnresolvedReason {
    /// Nothing serving the target app turned anything up.
    NoCandidates,
    /// There were candidates, the policy just didn't like any of them.
    Declined { candidates: usize },
}

#[derive(Debug, PartialEq, Clone)]
pub struct UnresolvedExtension {
    pub extension: Arc<Extension>,
    pub reason: UnresolvedReason,
}

/// A backup's extension table, resolved against a target app.
#[derive(Debug, PartialEq, Clone)]
pub struct ExtensionMapping {
    target: ReaderID,
    resolved: HashMap<Arc<Extension>, Arc<Extension>>,
    unresolved: Vec<UnresolvedExtension>,
}

impl ExtensionMapping {
    pub fn new(target: ReaderID) -> Self {
        Self { target, resolved: HashMap::new(), unresolved: Vec::new() }
    }

    pub fn target(&self) -> ReaderID {
        self.target
    }

    pub fn insert(&mut self, source: Arc<Extension>, target: Arc<Extension>) {
        self.resolved.insert(source, target);
    }

    pub fn mark_unresolved(&mut self, extension: Arc<Extension>, reason: UnresolvedReason) {
        self.unresolved.push(UnresolvedExtension { extension, reason });
    }

    pub fn get(&self, source: &Arc<Extension>) -> Option<&Arc<Extension>> {
        self.resolved.get(source)
    }

    /// Whether resolution looked at `source` at all, mapped or not. An entry
    /// whose extension isn't in here was never in `Backup::extensions` either.
    pub fn contains(&self, source: &Arc<Extension>) -> bool {
        self.resolved.contains_key(source)
            || self.unresolved.iter().any(|u| &u.extension == source)
    }

    pub fn resolved(&self) -> impl Iterator<Item = (&Arc<Extension>, &Arc<Extension>)> {
        self.resolved.iter()
    }

    pub fn unresolved(&self) -> &[UnresolvedExtension] {
        &self.unresolved
    }

    pub fn is_fully_resolved(&self) -> bool {
        self.unresolved.is_empty()
    }
}
