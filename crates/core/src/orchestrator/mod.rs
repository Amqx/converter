pub mod mapping;
pub mod policy;

use thiserror::Error;

use crate::ReaderID;
use crate::extensions::ExtensionError;
use crate::format::BackupFormat;
use crate::extensions::matcher::{ExtensionMatcher, RefreshReport};
use crate::extensions::types::ExtensionQuery;
use crate::io::reader::{BackupReader, ReadReport};
use crate::io::writer::{BackupWriter, WriteReport};
use crate::io::{ReadError, WriteError};
use crate::ir::types::{Backup, Manga};

use mapping::{ExtensionMapping, UnresolvedReason};
use policy::{Resolution, ResolutionPolicy};

#[derive(Error, Debug)]
pub enum OrchestratorError {
    #[error("no reader registered for {0}")]
    NoReaderFor(ReaderID),
    #[error("no writer registered for {0}")]
    NoWriterFor(ReaderID),
    #[error("mapping targets {mapping} but the backup is being written for {writer}")]
    MappingTargetMismatch { mapping: ReaderID, writer: ReaderID },
    #[error("resolution policy aborted on extension '{extension}'")]
    ResolutionAborted { extension: String },
    #[error(
        "a repository serving {expected} returned extension '{extension}', \
         which belongs to {found}"
    )]
    ForeignExtension { extension: String, expected: ReaderID, found: ReaderID },
    #[error(
        "entry '{entry}' references extension '{extension}', which is not in the \
         backup's extension table; the reader must share one `Arc<Extension>` \
         between `Backup::extensions` and the entries that use it"
    )]
    UnknownEntryExtension { entry: String, extension: String },
    #[error(transparent)]
    Extensions(#[from] ExtensionError),
    #[error(transparent)]
    Read(#[from] ReadError),
    #[error(transparent)]
    Write(#[from] WriteError),
}

pub type OrchestratorResult<T> = Result<T, OrchestratorError>;

/// What [`Orchestrator::prune`] threw away.
#[derive(Debug, PartialEq, Clone, Default)]
pub struct PruneReport {
    /// The target app has nothing to map these onto. Expected loss; see
    /// [`ExtensionMapping::unresolved`] for why each one failed.
    pub unresolved: Vec<Manga>,
    /// Entries pointing at an extension that was never in `Backup::extensions`.
    /// Resolution only ever looks at that table, so these never got a chance -
    /// which makes it a reader bug rather than a missing counterpart.
    pub unknown: Vec<Manga>,
}

impl PruneReport {
    pub fn dropped(&self) -> usize {
        self.unresolved.len() + self.unknown.len()
    }
}

/// Everything one conversion produced.
#[derive(Debug)]
pub struct Conversion {
    pub bytes: Vec<u8>,
    pub mapping: ExtensionMapping,
    pub read: ReadReport,
    pub prune: PruneReport,
    pub write: WriteReport,
}

/// Drives one backup from the source app to the target app.
pub struct Orchestrator {
    readers: Vec<Box<dyn BackupReader>>,
    writers: Vec<Box<dyn BackupWriter>>,
    matcher: ExtensionMatcher,
    policy: Box<dyn ResolutionPolicy>,
}

impl Orchestrator {
    pub fn new(matcher: ExtensionMatcher, policy: Box<dyn ResolutionPolicy>) -> Self {
        Self { readers: Vec::new(), writers: Vec::new(), matcher, policy }
    }

    pub fn register_reader(&mut self, reader: Box<dyn BackupReader>) {
        self.readers.push(reader);
    }

    pub fn register_writer(&mut self, writer: Box<dyn BackupWriter>) {
        self.writers.push(writer);
    }

    /// Reader, writer and repositories in one go.
    pub fn register_format<F: BackupFormat>(&mut self) {
        let reader = F::reader();
        assert_eq!(
            reader.reader(),
            F::id(),
            "reader reports a different format than it was registered under; \
             check the `format_id!` in its `BackupReader` impl"
        );
        self.register_reader(reader);

        if let Some(writer) = F::writer() {
            assert_eq!(
                writer.reader(),
                F::id(),
                "writer reports a different format than it was registered under; \
                 check the `format_id!` in its `BackupWriter` impl"
            );
            self.register_writer(writer);
        }
        for repo in F::repositories() {
            self.matcher.register_repo(repo);
        }
    }

    /// Brings every repository's index up to date. Worth checking
    /// [`RefreshReport::is_complete`] first - after a partial refresh, plenty
    /// of extensions look missing that are only unindexed.
    pub async fn refresh(&mut self) -> RefreshReport {
        self.matcher.refresh().await
    }

    pub fn matcher(&self) -> &ExtensionMatcher {
        &self.matcher
    }

    /// Parses `input` into the IR.
    pub fn read(&self, input: &[u8], from: ReaderID) -> OrchestratorResult<(Backup, ReadReport)> {
        let reader = self
            .readers
            .iter()
            .find(|r| r.reader() == from)
            .ok_or(OrchestratorError::NoReaderFor(from))?;
        Ok(reader.read_backup(input)?)
    }

    /// Pairs each of the backup's extensions with its counterpart in `to`.
    pub fn resolve(&self, backup: &Backup, to: ReaderID) -> OrchestratorResult<ExtensionMapping> {
        let mut mapping = ExtensionMapping::new(to);

        for extension in &backup.extensions {
            let query = ExtensionQuery::equivalent_of(extension, to);
            let candidates = self.matcher.find_matches(&query)?;

            match self.policy.resolve(extension, &candidates) {
                Resolution::Use(target) => {
                    // find_matches filters on the repo's declared app, not the
                    // extension's own, so a repo covering several apps can hand
                    // back a foreign one. Writing that out gives the user a
                    // backup pointing at an extension they can't install.
                    if target.reader != to {
                        return Err(OrchestratorError::ForeignExtension {
                            extension: target.name.clone(),
                            expected: to,
                            found: target.reader,
                        });
                    }
                    mapping.insert(extension.clone(), target)
                }
                Resolution::Skip => {
                    let reason = if candidates.is_empty() {
                        UnresolvedReason::NoCandidates
                    } else {
                        UnresolvedReason::Declined { candidates: candidates.len() }
                    };
                    mapping.mark_unresolved(extension.clone(), reason);
                }
                Resolution::Abort => {
                    return Err(OrchestratorError::ResolutionAborted {
                        extension: extension.name.clone(),
                    });
                }
            }
        }

        Ok(mapping)
    }

    /// Drops entries whose extension went unresolved, plus the extensions left
    /// with nothing pointing at them.
    ///
    /// Entries resolution never saw go too, but land in
    /// [`PruneReport::unknown`] instead, because quietly handing back a short
    /// backup would bury the reader bug behind them.
    pub fn prune(&self, backup: &mut Backup, mapping: &ExtensionMapping) -> PruneReport {
        let mut report = PruneReport::default();
        let mut kept = Vec::with_capacity(backup.entries.len());

        for entry in backup.entries.drain(..) {
            if mapping.get(&entry.extension).is_some() {
                kept.push(entry);
            } else if mapping.contains(&entry.extension) {
                report.unresolved.push(entry);
            } else {
                report.unknown.push(entry);
            }
        }

        backup.entries = kept;
        backup.extensions.retain(|ext| mapping.get(ext).is_some());

        report
    }

    /// Serializes the backup for the target app.
    pub fn write(
        &self,
        backup: &Backup,
        mapping: &ExtensionMapping,
        to: ReaderID,
    ) -> OrchestratorResult<(Vec<u8>, WriteReport)> {
        if mapping.target() != to {
            return Err(OrchestratorError::MappingTargetMismatch {
                mapping: mapping.target(),
                writer: to,
            });
        }

        let writer = self
            .writers
            .iter()
            .find(|w| w.reader() == to)
            .ok_or(OrchestratorError::NoWriterFor(to))?;
        Ok(writer.write_backup(backup, mapping)?)
    }

    /// read -> resolve -> prune -> write.
    pub fn convert(
        &self,
        input: &[u8],
        from: ReaderID,
        to: ReaderID,
    ) -> OrchestratorResult<Conversion> {
        let (mut backup, read) = self.read(input, from)?;
        let mapping = self.resolve(&backup, to)?;
        let prune = self.prune(&mut backup, &mapping);

        if let Some(entry) = prune.unknown.first() {
            return Err(OrchestratorError::UnknownEntryExtension {
                entry: entry.title.clone(),
                extension: entry.extension.name.clone(),
            });
        }

        let (bytes, write) = self.write(&backup, &mapping, to)?;

        Ok(Conversion { bytes, mapping, read, prune, write })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::policy::{AutoBest, Strict};
    use super::*;
    use crate::extensions::ExtResult;
    use crate::extensions::lang::Language;
    use crate::extensions::types::{
        Extension, ExtensionMatch, ExtensionRepository, ExtensionRepositoryInfo,
    };
    use crate::ir::types::MangaStatus;

    const TACHI: ReaderID = ReaderID("tachiyomi");
    const AIDOKU: ReaderID = ReaderID("aidoku");

    fn repo_info(reader: ReaderID) -> ExtensionRepositoryInfo {
        ExtensionRepositoryInfo {
            reader,
            name: Some("test repo".into()),
            short_name: None,
            website: None,
            signing_key: None,
        }
    }

    fn extension(reader: ReaderID, name: &str, external_id: &str) -> Arc<Extension> {
        Arc::new(Extension {
            reader,
            name: name.into(),
            external_id: external_id.into(),
            url: format!("https://example.test/{external_id}"),
            lang: Language::from_code("en").unwrap(),
            repo: repo_info(reader),
        })
    }

    fn manga(title: &str, extension: Arc<Extension>) -> Manga {
        Manga {
            id: title.into(),
            title: title.into(),
            extension,
            status: MangaStatus::Unknown,
            nsfw: false,
            favorite: true,
            author: None,
            artist: None,
            description: None,
            chapters: Vec::new(),
            history: Vec::new(),
            tags: Vec::new(),
            categories: Vec::new(),
            cover_url: None,
            date_added: None,
        }
    }

    /// Exact on external_id, `name_score` on the name alone.
    struct StubRepo {
        info: ExtensionRepositoryInfo,
        index: Vec<Arc<Extension>>,
        /// Real repos disagree wildly on how much a name is worth, some
        /// treating it as good as an id.
        name_score: f32,
    }

    impl ExtensionRepository for StubRepo {
        fn info(&self) -> &ExtensionRepositoryInfo {
            &self.info
        }

        fn search(&self, query: &ExtensionQuery) -> ExtResult<Vec<ExtensionMatch>> {
            Ok(self
                .index
                .iter()
                .filter_map(|candidate| {
                    let score = if query.external_id.as_deref() == Some(&candidate.external_id) {
                        1.0
                    } else if query.name.as_deref() == Some(&candidate.name) {
                        self.name_score
                    } else {
                        return None;
                    };
                    Some(ExtensionMatch { extension: candidate.clone(), score })
                })
                .collect())
        }
    }

    struct StubReader {
        backup: Backup,
    }

    impl BackupReader for StubReader {
        fn reader(&self) -> ReaderID {
            TACHI
        }

        fn read_backup(&self, _bytes: &[u8]) -> Result<(Backup, ReadReport), ReadError> {
            Ok((self.backup.clone(), ReadReport::default()))
        }
    }

    /// One `title:target_external_id` per entry, so a test can check both what
    /// survived and where it was pointed.
    struct StubWriter;

    impl BackupWriter for StubWriter {
        fn reader(&self) -> ReaderID {
            AIDOKU
        }

        fn write_backup(
            &self,
            backup: &Backup,
            mapping: &ExtensionMapping,
        ) -> Result<(Vec<u8>, WriteReport), WriteError> {
            let body = backup
                .entries
                .iter()
                .map(|entry| {
                    let target = mapping
                        .get(&entry.extension)
                        .expect("prune should have removed unmapped entries");
                    format!("{}:{}", entry.title, target.external_id)
                })
                .collect::<Vec<_>>()
                .join(",");
            Ok((body.into_bytes(), WriteReport::default()))
        }
    }

    /// Two sources, one of which Aidoku has no counterpart for.
    fn fixture() -> (Backup, Arc<Extension>, Arc<Extension>) {
        let matched = extension(TACHI, "MangaDex", "mangadex");
        let orphan = extension(TACHI, "DeadSource", "deadsource");
        let backup = Backup {
            reader: TACHI,
            version: Some("2".into()),
            extensions: vec![matched.clone(), orphan.clone()],
            entries: vec![
                manga("Berserk", matched.clone()),
                manga("Vagabond", orphan.clone()),
            ],
            categories: Vec::new(),
        };
        (backup, matched, orphan)
    }

    fn orchestrator(index: Vec<Arc<Extension>>, policy: Box<dyn ResolutionPolicy>) -> Orchestrator {
        orchestrator_scoring_names(index, policy, 0.9)
    }

    fn orchestrator_scoring_names(
        index: Vec<Arc<Extension>>,
        policy: Box<dyn ResolutionPolicy>,
        name_score: f32,
    ) -> Orchestrator {
        let mut matcher = ExtensionMatcher::default();
        matcher.register_repo(Box::new(StubRepo {
            info: repo_info(AIDOKU),
            index,
            name_score,
        }));

        let (backup, _, _) = fixture();
        let mut orchestrator = Orchestrator::new(matcher, policy);
        orchestrator.register_reader(Box::new(StubReader { backup }));
        orchestrator.register_writer(Box::new(StubWriter));
        orchestrator
    }

    #[test]
    fn converts_what_it_can_and_drops_the_rest() {
        let orchestrator = orchestrator(
            vec![extension(AIDOKU, "MangaDex", "mangadex")],
            Box::new(AutoBest::default()),
        );

        let conversion = orchestrator.convert(b"", TACHI, AIDOKU).unwrap();

        assert_eq!(String::from_utf8(conversion.bytes).unwrap(), "Berserk:mangadex");
        assert_eq!(conversion.prune.dropped(), 1);
        assert_eq!(conversion.prune.unresolved[0].title, "Vagabond");
        assert!(conversion.prune.unknown.is_empty());
        assert!(!conversion.mapping.is_fully_resolved());
    }

    #[test]
    fn records_why_an_extension_failed() {
        let orchestrator = orchestrator(
            vec![extension(AIDOKU, "MangaDex", "mangadex")],
            Box::new(AutoBest::default()),
        );
        let (backup, _, orphan) = fixture();

        let mapping = orchestrator.resolve(&backup, AIDOKU).unwrap();

        assert_eq!(mapping.unresolved().len(), 1);
        assert_eq!(mapping.unresolved()[0].extension, orphan);
        assert_eq!(mapping.unresolved()[0].reason, UnresolvedReason::NoCandidates);
    }

    #[test]
    fn below_threshold_match_is_declined() {
        // Name-only scores 0.9 here, so ask for more than that.
        let orchestrator = orchestrator(
            vec![extension(AIDOKU, "MangaDex", "md-different-id")],
            Box::new(AutoBest { min_score: 0.95 }),
        );
        let (backup, matched, _) = fixture();

        let mapping = orchestrator.resolve(&backup, AIDOKU).unwrap();

        assert!(mapping.get(&matched).is_none());
        assert_eq!(
            mapping.unresolved()[0].reason,
            UnresolvedReason::Declined { candidates: 1 }
        );
    }

    #[test]
    fn prune_takes_orphaned_extensions_too() {
        let orchestrator = orchestrator(
            vec![extension(AIDOKU, "MangaDex", "mangadex")],
            Box::new(AutoBest::default()),
        );
        let (mut backup, matched, _) = fixture();
        let mapping = orchestrator.resolve(&backup, AIDOKU).unwrap();

        orchestrator.prune(&mut backup, &mapping);

        assert_eq!(backup.extensions, vec![matched]);
    }

    #[test]
    fn strict_aborts_on_two_exact_matches() {
        // Two genuinely different extensions, both exact. Two rows sharing an
        // external_id would be the same extension served twice, and the matcher
        // collapses those long before the policy sees them.
        let orchestrator = orchestrator_scoring_names(
            vec![
                extension(AIDOKU, "MangaDex", "mangadex"),
                extension(AIDOKU, "MangaDex", "mangadex-mirror"),
            ],
            Box::new(Strict),
            1.0,
        );
        let (backup, _, _) = fixture();

        assert!(matches!(
            orchestrator.resolve(&backup, AIDOKU),
            Err(OrchestratorError::ResolutionAborted { .. })
        ));
    }

    #[test]
    fn rejects_a_mapping_for_another_app() {
        let orchestrator = orchestrator(Vec::new(), Box::new(AutoBest::default()));
        let (backup, _, _) = fixture();
        let mapping = ExtensionMapping::new(TACHI);

        assert!(matches!(
            orchestrator.write(&backup, &mapping, AIDOKU),
            Err(OrchestratorError::MappingTargetMismatch { .. })
        ));
    }

    #[test]
    fn unregistered_app_errors_instead_of_panicking() {
        let orchestrator = orchestrator(Vec::new(), Box::new(AutoBest::default()));

        assert!(matches!(
            orchestrator.read(b"", AIDOKU),
            Err(OrchestratorError::NoReaderFor(_))
        ));
    }

    #[test]
    fn entry_outside_the_extension_table_is_reported_separately() {
        // A reader that rebuilds an entry's `Extension` instead of sharing the
        // registered one: same source, different value, different hash.
        let orchestrator = orchestrator(
            vec![extension(AIDOKU, "MangaDex", "mangadex")],
            Box::new(AutoBest::default()),
        );
        let (mut backup, matched, _) = fixture();
        let rebuilt = Arc::new(Extension { url: String::new(), ..(*matched).clone() });
        backup.entries.push(manga("Monster", rebuilt));

        let mapping = orchestrator.resolve(&backup, AIDOKU).unwrap();
        let report = orchestrator.prune(&mut backup, &mapping);

        assert_eq!(report.dropped(), 2);
        assert_eq!(report.unresolved.len(), 1, "the orphan source");
        assert_eq!(report.unknown.len(), 1, "the rebuilt extension");
        assert_eq!(report.unknown[0].title, "Monster");
    }

    #[test]
    fn convert_refuses_to_silently_drop_unknown_extensions() {
        let matched = extension(TACHI, "MangaDex", "mangadex");
        let rebuilt = Arc::new(Extension { url: String::new(), ..(*matched).clone() });
        // Every entry points outside the table, so a silent prune hands back an
        // empty backup with a mapping claiming everything went fine.
        let backup = Backup {
            reader: TACHI,
            version: None,
            extensions: vec![matched],
            entries: vec![manga("Berserk", rebuilt)],
            categories: Vec::new(),
        };

        let mut matcher = ExtensionMatcher::default();
        matcher.register_repo(Box::new(StubRepo {
            info: repo_info(AIDOKU),
            index: vec![extension(AIDOKU, "MangaDex", "mangadex")],
            name_score: 0.9,
        }));
        let mut orchestrator =
            Orchestrator::new(matcher, Box::new(AutoBest::default()));
        orchestrator.register_reader(Box::new(StubReader { backup }));
        orchestrator.register_writer(Box::new(StubWriter));

        assert!(matches!(
            orchestrator.convert(b"", TACHI, AIDOKU),
            Err(OrchestratorError::UnknownEntryExtension { .. })
        ));
    }

    #[test]
    fn candidate_from_another_app_is_rejected() {
        // Registered as an Aidoku repo, but indexing a Tachiyomi extension.
        let orchestrator = orchestrator(
            vec![extension(TACHI, "MangaDex", "mangadex")],
            Box::new(AutoBest::default()),
        );
        let (backup, _, _) = fixture();

        assert!(matches!(
            orchestrator.resolve(&backup, AIDOKU),
            Err(OrchestratorError::ForeignExtension { expected: AIDOKU, found: TACHI, .. })
        ));
    }
}
