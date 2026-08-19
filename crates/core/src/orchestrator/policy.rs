use std::sync::Arc;

use crate::extensions::types::{Extension, ExtensionMatch};

/// What to do with one source extension, given whatever candidates turned up.
#[derive(Debug, PartialEq, Clone)]
pub enum Resolution {
    Use(Arc<Extension>),
    /// Drop every entry from this source, noting it in the mapping.
    Skip,
    /// Give up on the whole conversion.
    Abort,
}

pub trait ResolutionPolicy {
    /// `candidates` comes in best-first, and is often empty.
    fn resolve(&self, source: &Extension, candidates: &[ExtensionMatch]) -> Resolution;
}

/// Best candidate at or above `min_score`; skips the source if nothing clears
/// the bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoBest {
    pub min_score: f32,
}

impl Default for AutoBest {
    fn default() -> Self {
        Self { min_score: 0.75 }
    }
}

impl ResolutionPolicy for AutoBest {
    fn resolve(&self, _source: &Extension, candidates: &[ExtensionMatch]) -> Resolution {
        match candidates.first() {
            Some(best) if best.score >= self.min_score => Resolution::Use(best.extension.clone()),
            _ => Resolution::Skip,
        }
    }
}

/// Exact match or nothing. If two are exact we'd rather stop than guess.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Strict;

impl ResolutionPolicy for Strict {
    fn resolve(&self, _source: &Extension, candidates: &[ExtensionMatch]) -> Resolution {
        let exact: Vec<&ExtensionMatch> =
            candidates.iter().filter(|c| c.score >= 1.0).collect();
        match exact.as_slice() {
            [only] => Resolution::Use(only.extension.clone()),
            [] => Resolution::Skip,
            _ => Resolution::Abort,
        }
    }
}
