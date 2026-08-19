use std::collections::HashSet;

use crate::extensions::types::{
    ExtensionMatch, ExtensionQuery, ExtensionRepository, ExtensionRepositoryInfo,
};
use crate::extensions::{ExtResult, ExtensionError};

#[derive(Debug)]
pub struct FailedRepository {
    pub repo: ExtensionRepositoryInfo,
    pub error: ExtensionError,
}

/// One repo failing doesn't stop the rest, so both lists can be non-empty.
#[derive(Debug, Default)]
pub struct RefreshReport {
    pub refreshed: Vec<ExtensionRepositoryInfo>,
    pub failed: Vec<FailedRepository>,
}

impl RefreshReport {
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

#[derive(Default)]
pub struct ExtensionMatcher {
    extension_repos: Vec<Box<dyn ExtensionRepository>>,
}

impl ExtensionMatcher {
    pub fn register_repo(&mut self, repo: Box<dyn ExtensionRepository>) {
        self.extension_repos.push(repo);
    }

    pub fn registered(&self) -> Vec<&ExtensionRepositoryInfo> {
        self.extension_repos.iter().map(|r| r.info()).collect()
    }

    /// Refreshes everything registered. One flaky host shouldn't cost us the
    /// whole index, so failures end up in the report instead of aborting.
    pub async fn refresh(&mut self) -> RefreshReport {
        let mut report = RefreshReport::default();

        for repo in &mut self.extension_repos {
            match repo.refresh().await {
                Ok(()) => report.refreshed.push(repo.info().clone()),
                Err(error) => {
                    report.failed.push(FailedRepository { repo: repo.info().clone(), error })
                }
            }
        }

        report
    }

    /// Candidates from every repo serving `query.reader`, best score first.
    pub fn find_matches(&self, query: &ExtensionQuery) -> ExtResult<Vec<ExtensionMatch>> {
        let mut results = Vec::new();

        for repo in &self.extension_repos {
            if repo.info().reader != query.reader {
                continue;
            }
            results.extend(repo.search(query)?);
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Two repos indexing the same extension always describe it a little
        // differently (`repo` alone guarantees that), so dedup on its identity
        // rather than the whole struct. Without an id we can't tell two sources
        // apart, so keep both.
        let mut seen = HashSet::new();
        results.retain(|m| {
            m.extension.external_id.is_empty()
                || seen.insert((m.extension.reader, m.extension.external_id.clone()))
        });

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    use super::*;
    use crate::ReaderID;
    use crate::extensions::lang::Language;
    use crate::extensions::types::{Extension, RefreshFuture};

    const TACHI: ReaderID = ReaderID("tachiyomi");
    const AIDOKU: ReaderID = ReaderID("aidoku");

    /// Saves pulling a whole runtime in just to poll the stubs below once.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = Box::pin(future);
        let mut cx = Context::from_waker(Waker::noop());
        match Pin::new(&mut future).poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("test futures never pend"),
        }
    }

    fn repo_info(reader: ReaderID, name: &str) -> ExtensionRepositoryInfo {
        ExtensionRepositoryInfo {
            reader,
            name: Some(name.into()),
            short_name: None,
            website: None,
            signing_key: None,
        }
    }

    fn extension(repo: &ExtensionRepositoryInfo, name: &str, external_id: &str) -> Arc<Extension> {
        Arc::new(Extension {
            reader: repo.reader,
            name: name.into(),
            external_id: external_id.into(),
            url: format!("https://example.test/{external_id}"),
            lang: Language::from_code("en").unwrap(),
            repo: repo.clone(),
        })
    }

    /// Hands back its whole index at a fixed score.
    struct StubRepo {
        info: ExtensionRepositoryInfo,
        index: Vec<Arc<Extension>>,
        score: f32,
        refresh_fails: bool,
        refreshed: bool,
    }

    impl StubRepo {
        fn new(info: ExtensionRepositoryInfo, index: Vec<Arc<Extension>>) -> Self {
            Self { info, index, score: 1.0, refresh_fails: false, refreshed: false }
        }

        fn failing(info: ExtensionRepositoryInfo) -> Self {
            Self { refresh_fails: true, ..Self::new(info, Vec::new()) }
        }

        fn scoring(mut self, score: f32) -> Self {
            self.score = score;
            self
        }
    }

    impl ExtensionRepository for StubRepo {
        fn info(&self) -> &ExtensionRepositoryInfo {
            &self.info
        }

        fn search(&self, _query: &ExtensionQuery) -> ExtResult<Vec<ExtensionMatch>> {
            Ok(self
                .index
                .iter()
                .map(|e| ExtensionMatch { extension: e.clone(), score: self.score })
                .collect())
        }

        fn refresh(&mut self) -> RefreshFuture<'_> {
            Box::pin(async move {
                if self.refresh_fails {
                    return Err(ExtensionError::UnknownLanguageCode("boom".into()));
                }
                self.refreshed = true;
                Ok(())
            })
        }
    }

    #[test]
    fn same_extension_from_two_repos_dedups() {
        let official = repo_info(AIDOKU, "official");
        let community = repo_info(AIDOKU, "community");
        let mut matcher = ExtensionMatcher::default();
        matcher.register_repo(Box::new(StubRepo::new(
            official.clone(),
            vec![extension(&official, "MangaDex", "mangadex")],
        )));
        matcher.register_repo(Box::new(StubRepo::new(
            community.clone(),
            vec![extension(&community, "MangaDex", "mangadex")],
        )));

        let matches = matcher.find_matches(&ExtensionQuery::new(AIDOKU)).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].extension.repo, official);
    }

    #[test]
    fn dedup_keeps_the_best_copy() {
        let weak = repo_info(AIDOKU, "weak");
        let strong = repo_info(AIDOKU, "strong");
        let mut matcher = ExtensionMatcher::default();
        matcher.register_repo(Box::new(
            StubRepo::new(weak.clone(), vec![extension(&weak, "MangaDex", "mangadex")])
                .scoring(0.5),
        ));
        matcher.register_repo(Box::new(StubRepo::new(
            strong.clone(),
            vec![extension(&strong, "MangaDex", "mangadex")],
        )));

        let matches = matcher.find_matches(&ExtensionQuery::new(AIDOKU)).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].score, 1.0);
        assert_eq!(matches[0].extension.repo, strong);
    }

    #[test]
    fn different_extensions_survive_dedup() {
        let repo = repo_info(AIDOKU, "official");
        let mut matcher = ExtensionMatcher::default();
        matcher.register_repo(Box::new(StubRepo::new(
            repo.clone(),
            vec![
                extension(&repo, "MangaDex", "mangadex"),
                extension(&repo, "Comick", "comick"),
            ],
        )));

        assert_eq!(matcher.find_matches(&ExtensionQuery::new(AIDOKU)).unwrap().len(), 2);
    }

    #[test]
    fn skips_repos_for_other_apps() {
        let tachi = repo_info(TACHI, "tachi repo");
        let mut matcher = ExtensionMatcher::default();
        matcher.register_repo(Box::new(StubRepo::new(
            tachi.clone(),
            vec![extension(&tachi, "MangaDex", "mangadex")],
        )));

        assert!(matcher.find_matches(&ExtensionQuery::new(AIDOKU)).unwrap().is_empty());
    }

    #[test]
    fn a_failing_repo_does_not_stop_the_rest() {
        let broken = repo_info(AIDOKU, "broken");
        let working = repo_info(AIDOKU, "working");
        let mut matcher = ExtensionMatcher::default();
        matcher.register_repo(Box::new(StubRepo::failing(broken.clone())));
        matcher.register_repo(Box::new(StubRepo::new(
            working.clone(),
            vec![extension(&working, "MangaDex", "mangadex")],
        )));

        let report = block_on(matcher.refresh());

        assert!(!report.is_complete());
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].repo, broken);
        assert_eq!(report.refreshed, vec![working]);
        // ...and the one queued behind it still ran.
        assert_eq!(matcher.find_matches(&ExtensionQuery::new(AIDOKU)).unwrap().len(), 1);
    }

    #[test]
    fn clean_refresh_reports_every_repo() {
        let mut matcher = ExtensionMatcher::default();
        matcher.register_repo(Box::new(StubRepo::new(repo_info(AIDOKU, "a"), Vec::new())));
        matcher.register_repo(Box::new(StubRepo::new(repo_info(AIDOKU, "b"), Vec::new())));

        let report = block_on(matcher.refresh());

        assert!(report.is_complete());
        assert_eq!(report.refreshed.len(), 2);
    }
}
