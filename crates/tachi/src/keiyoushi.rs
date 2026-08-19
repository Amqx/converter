use std::sync::Arc;
use std::time::Duration;

use converter_core::extensions::lang::Language;
use converter_core::extensions::types::{
    Extension, ExtensionMatch, ExtensionQuery, ExtensionRepository, ExtensionRepositoryInfo,
    RefreshFuture,
};
use converter_core::extensions::{ExtResult, ExtensionError};
use serde::Deserialize;

use crate::Tachiyomi;

const MIN_SCORE: f32 = 0.35;
const MAX_INFERRED_SCORE: f32 = 0.95;
const NEUTRAL_SCORE: f32 = 0.5;
const URL_AGREEMENT: f32 = 0.6;
const URL_DISAGREEMENT: f32 = 0.85;
/// How alike the names have to look before we trust a matching id
const ID_CORROBORATION: f32 = 0.4;

pub struct Keiyoushi {
    info: ExtensionRepositoryInfo,
    index: Vec<IndexedSource>,
}

struct IndexedSource {
    extension: Arc<Extension>,
    hosts: Vec<String>,
    name_key: String,
}

impl Keiyoushi {
    const URL: &str = "https://github.com/keiyoushi/extensions/raw/repo/index.json";
    const NAME: &str = "Keiyoushi";
    const TIMEOUT: Duration = Duration::from_secs(30);
    const USER_AGENT: &str = concat!("converter-tachi/", env!("CARGO_PKG_VERSION"));

    pub fn new() -> Self {
        Self {
            info: ExtensionRepositoryInfo::new(
                Tachiyomi::ID,
                Some("Keiyoushi".into()),
                Some("Keiyoushi".into()),
                Some("https://keiyoushi.github.io".into()),
                None,
            ),
            index: Vec::new(),
        }
    }

    async fn fetch(url: &str) -> ExtResult<String> {
        let client = reqwest::Client::builder()
            .user_agent(Self::USER_AGENT)
            .timeout(Self::TIMEOUT)
            .build()
            .map_err(|e| ExtensionError::Internal(e.to_string()))?;

        client
            .get(url)
            .send()
            .await
            .map_err(|e| ExtensionError::IndexUnreachable {
                repo: Self::NAME.to_string(),
                message: e.to_string(),
            })?
            .error_for_status()
            .map_err(|e| ExtensionError::IndexUnreachable {
                repo: Self::NAME.to_string(),
                message: e.to_string(),
            })?
            .text()
            .await
            .map_err(|e| ExtensionError::IndexUnreachable {
                repo: Self::NAME.to_string(),
                message: e.to_string(),
            })
    }

    /// Replaces the index with what Keiyoushi's json describes
    fn load(&mut self, json: &str) -> ExtResult<()> {
        let index: RepoIndex =
            serde_json::from_str(json).map_err(|error| ExtensionError::MalformedIndex {
                repo: Self::NAME.to_string(),
                message: error.to_string(),
            })?;

        let mut info = self.info.clone();
        info.name = non_empty(index.name).or_else(|| info.name.take());
        info.short_name = non_empty(index.badge_label).or_else(|| info.short_name.take());
        info.signing_key = non_empty(index.signing_key).or_else(|| info.signing_key.take());
        info.website = non_empty(index.contact.website).or_else(|| info.website.take());

        let sources: Vec<IndexedSource> = index
            .extension_list
            .extensions
            .into_iter()
            .flat_map(|extension| extension.sources)
            .filter_map(|source| Self::indexed(&info, source))
            .collect();

        if sources.is_empty() {
            return Err(ExtensionError::MalformedIndex {
                repo: Self::NAME.to_string(),
                message: "the index listed no usable sources; the schema has probably moved"
                    .to_string(),
            });
        }

        self.info = info;
        self.index = sources;

        Ok(())
    }

    /// Returns `None` for a source we couldn't map onto
    fn indexed(info: &ExtensionRepositoryInfo, source: IndexSource) -> Option<IndexedSource> {
        let external_id = source.id.into_string();
        if external_id.is_empty() || source.name.is_empty() {
            return None;
        }
        let lang = Language::from_code(&source.language).ok()?;

        let hosts = std::iter::once(&source.home_url)
            .chain(source.mirror_urls.iter())
            .filter_map(|url| host_of(url))
            .collect();

        Some(IndexedSource {
            name_key: name_key(&source.name),
            hosts,
            extension: Arc::new(Extension {
                reader: info.reader,
                name: source.name,
                external_id,
                url: source.home_url,
                lang,
                repo: info.clone(),
            }),
        })
    }
}

impl Default for Keiyoushi {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionRepository for Keiyoushi {
    fn info(&self) -> &ExtensionRepositoryInfo {
        &self.info
    }

    fn search(&self, query: &ExtensionQuery) -> ExtResult<Vec<ExtensionMatch>> {
        if query.reader != self.info.reader {
            return Ok(Vec::new());
        }

        let mut matches: Vec<ExtensionMatch> = self
            .index
            .iter()
            .filter_map(|source| {
                let score = score(source, query);
                (score >= MIN_SCORE).then(|| ExtensionMatch {
                    extension: Arc::clone(&source.extension),
                    score,
                })
            })
            .collect();

        // The matcher sorts too, but only across repos; ours has to arrive
        // best-first for a single-repo caller to get the same answer.
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.extension.name.cmp(&b.extension.name))
        });

        Ok(matches)
    }

    fn refresh(&mut self) -> RefreshFuture<'_> {
        Box::pin(async move {
            let body = Self::fetch(Self::URL).await?;
            self.load(&body)
        })
    }
}

// The host adjustments read as "move the score this fraction of the way to 1",
// which is the shape worth seeing; `mul_add` would hide it, and its extra
// precision means nothing to a heuristic that ends up clamped anyway.
#[allow(clippy::suboptimal_flops)]
fn score(source: &IndexedSource, query: &ExtensionQuery) -> f32 {
    let wanted_name = query
        .name
        .as_deref()
        .map(name_key)
        .filter(|key| !key.is_empty());
    let wanted_host = query.url.as_deref().and_then(host_of);

    if let Some(id) = &query.external_id
        && !id.is_empty()
        && *id == source.extension.external_id
        && wanted_name
            .as_deref()
            .is_none_or(|name| name_similarity(&source.name_key, name) >= ID_CORROBORATION)
    {
        return 1.0;
    }

    let mut score = wanted_name.as_ref().map_or(NEUTRAL_SCORE, |name| {
        name_similarity(&source.name_key, name)
    });

    if let Some(host) = &wanted_host {
        score = if source.hosts.iter().any(|known| known == host) {
            score + (1.0 - score) * URL_AGREEMENT
        } else if source.hosts.iter().any(|known| same_site(known, host)) {
            score + (1.0 - score) * URL_AGREEMENT * 0.5
        } else {
            score * URL_DISAGREEMENT
        };
    }

    score *= language_factor(&source.extension.lang, query.lang.as_ref());

    score.clamp(0.0, MAX_INFERRED_SCORE)
}

fn language_factor(source: &Language, wanted: Option<&Language>) -> f32 {
    let Some(wanted) = wanted else { return 1.0 };

    match (source, wanted) {
        (a, b) if a == b => 1.0,
        (Language::All, _) | (_, Language::All) => 0.95,
        (Language::Tagged { iso: a, .. }, Language::Tagged { iso: b, .. }) if a == b => 0.85,
        // Kept, but nowhere near where a policy would take it unattended.
        _ => 0.4,
    }
}

/// Lowercase alphanumerics only, minus the app prefix some repos put in front
fn name_key(name: &str) -> String {
    let trimmed = name.trim();
    let stripped = ["tachiyomi:", "mihon:", "aidoku:"]
        .iter()
        .find_map(|prefix| {
            trimmed
                .get(..prefix.len())
                .filter(|start| start.eq_ignore_ascii_case(prefix))
                .map(|_| &trimmed[prefix.len()..])
        })
        .unwrap_or(trimmed);

    stripped
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// 1.0 for identical names, tapering to 0.0 as the edit distance approaches the
/// length of the longer one.
fn name_similarity(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }

    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 0.0;
    }

    let distance = f32::from(u16::try_from(levenshtein(&a, &b)).unwrap_or(u16::MAX));
    let longest = f32::from(u16::try_from(longest).unwrap_or(u16::MAX));

    1.0 - distance / longest
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();

    for (i, from) in a.iter().enumerate() {
        let mut current = Vec::with_capacity(previous.len());
        current.push(i + 1);

        for ((to, &deletion), &substitution) in
            b.iter().zip(previous.iter().skip(1)).zip(previous.iter())
        {
            let insertion = current.last().copied().unwrap_or(i + 1);
            current.push(
                (substitution + usize::from(from != to))
                    .min(deletion + 1)
                    .min(insertion + 1),
            );
        }

        previous = current;
    }

    previous.last().copied().unwrap_or(0)
}

/// The bare host of a url
fn host_of(url: &str) -> Option<String> {
    let after_scheme = url
        .trim()
        .split_once("://")
        .map_or_else(|| url.trim(), |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host.split_once(':').map_or(host, |(host, _)| host);
    let host = host.trim_end_matches('.').to_lowercase();
    let host = host.trim_start_matches("www.");

    (!host.is_empty()).then(|| host.to_string())
}

const SECOND_LEVEL_SUFFIXES: [&str; 9] =
    ["com", "co", "net", "org", "edu", "gov", "ac", "ne", "or"];

/// Whether two hosts look like the same site wearing a different tld
fn same_site(a: &str, b: &str) -> bool {
    fn site_label(host: &str) -> Option<&str> {
        let mut labels = host.rsplit('.');
        labels.next()?;
        let candidate = labels.next()?;
        let label = if SECOND_LEVEL_SUFFIXES.contains(&candidate) {
            labels.next()?
        } else {
            candidate
        };

        (label.len() >= 3).then_some(label)
    }

    matches!((site_label(a), site_label(b)), (Some(a), Some(b)) if a == b)
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoIndex {
    name: Option<String>,
    badge_label: Option<String>,
    signing_key: Option<String>,
    #[serde(default)]
    contact: Contact,
    extension_list: ExtensionList,
}

#[derive(Debug, Default, Deserialize)]
struct Contact {
    website: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExtensionList {
    extensions: Vec<IndexExtension>,
}

#[derive(Debug, Deserialize)]
struct IndexExtension {
    #[serde(default)]
    sources: Vec<IndexSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexSource {
    id: SourceId,
    name: String,
    language: String,
    home_url: String,
    #[serde(default)]
    mirror_urls: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SourceId {
    Text(String),
    Number(u64),
}

impl SourceId {
    fn into_string(self) -> String {
        match self {
            Self::Text(id) => id.trim().to_string(),
            Self::Number(id) => id.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use converter_core::orchestrator::policy::AutoBest;

    const INDEX: &str = r#"{
      "name": "Keiyoushi",
      "badgeLabel": "KEI",
      "signingKey": "9add655a78e96c4ec7a53ef89dccb557cb5d767489fac5e785d671a5a75d4da2",
      "contact": { "website": "https://keiyoushi.github.io", "discord": "https://discord.gg/x" },
      "extensionList": { "extensions": [
        {
          "name": "MangaDex",
          "packageName": "eu.kanade.tachiyomi.extension.all.mangadex",
          "versionName": "1.4.238",
          "sources": [
            { "id": "2499283573021220255", "name": "MangaDex", "language": "en",
              "homeUrl": "https://mangadex.org" },
            { "id": "9", "name": "MangaDex", "language": "ru",
              "homeUrl": "https://mangadex.org" }
          ]
        },
        {
          "name": "Comick (Unoriginal)",
          "packageName": "eu.kanade.tachiyomi.extension.all.comicklive",
          "sources": [
            { "id": "4972933717624256217", "name": "Comick (Unoriginal)", "language": "en",
              "homeUrl": "https://comick.live",
              "mirrorUrls": ["https://comick.live", "https://comick.art"] }
          ]
        },
        {
          "name": "Unusable",
          "packageName": "eu.kanade.tachiyomi.extension.all.unusable",
          "sources": [
            { "id": "77", "name": "Nowhere", "language": "other",
              "homeUrl": "https://nowhere.test" },
            { "id": "", "name": "No Id", "language": "en", "homeUrl": "https://noid.test" }
          ]
        }
      ]}
    }"#;

    fn loaded() -> Keiyoushi {
        let mut repo = Keiyoushi::new();
        repo.load(INDEX).expect("the fixture parses");
        repo
    }

    fn query() -> ExtensionQuery {
        ExtensionQuery::new(Tachiyomi::ID)
    }

    fn best(repo: &Keiyoushi, query: &ExtensionQuery) -> ExtensionMatch {
        repo.search(query)
            .unwrap()
            .into_iter()
            .next()
            .expect("a candidate")
    }

    /// The bar `AutoBest` holds candidates to by default.
    fn auto_bar() -> f32 {
        AutoBest::default().min_score
    }

    #[test]
    fn indexes_one_extension_per_source() {
        // Three usable sources across three packages; MangaDex ships two.
        let matches = loaded().search(&query()).unwrap();
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn drops_sources_it_could_not_map_onto() {
        let repo = loaded();
        // "other" isn't a language, and an id-less source can't go in a backup.
        assert!(repo.index.iter().all(|s| s.extension.name != "Nowhere"));
        assert!(
            repo.index
                .iter()
                .all(|s| !s.extension.external_id.is_empty())
        );
    }

    #[test]
    fn repository_metadata_comes_from_the_index() {
        let info = loaded().info().clone();
        assert_eq!(info.reader, Tachiyomi::ID);
        assert_eq!(info.short_name.as_deref(), Some("KEI"));
        assert_eq!(info.website.as_deref(), Some("https://keiyoushi.github.io"));
        assert!(
            info.signing_key.is_some(),
            "the key only ever arrives with the index"
        );
    }

    #[test]
    fn extensions_carry_the_refreshed_repo_info() {
        let repo = loaded();
        assert_eq!(&repo.index[0].extension.repo, repo.info());
    }

    #[test]
    fn a_matching_id_scores_exactly_one() {
        let mut query = query();
        query.external_id = Some("2499283573021220255".into());

        let best = best(&loaded(), &query);
        assert_eq!(best.score, 1.0, "Strict only accepts 1.0");
        assert_eq!(best.extension.name, "MangaDex");
    }

    #[test]
    fn only_an_id_reaches_one() {
        let mut query = query();
        query.name = Some("MangaDex".into());
        query.url = Some("https://mangadex.org".into());
        query.lang = Some(Language::from_code("en").unwrap());

        let best = best(&loaded(), &query);
        assert!(
            best.score >= auto_bar(),
            "{} should clear the auto bar",
            best.score
        );
        assert!(best.score < 1.0, "an inferred match must not read as exact");
    }

    #[test]
    fn a_renamed_domain_still_matches_on_the_name() {
        let mut query = query();
        query.name = Some("MangaDex".into());
        query.url = Some("https://mangadex.moved.example".into());
        query.lang = Some(Language::from_code("en").unwrap());

        let best = best(&loaded(), &query);
        assert!(
            best.score >= auto_bar(),
            "{} should still clear the bar",
            best.score
        );
    }

    #[test]
    fn a_mirror_counts_as_the_source() {
        let mut query = query();
        query.name = Some("Comick".into());
        query.url = Some("https://comick.art/some/path".into());

        let best = best(&loaded(), &query);
        assert_eq!(best.extension.name, "Comick (Unoriginal)");
        assert!(
            best.score >= auto_bar(),
            "{} should clear the auto bar",
            best.score
        );
    }

    #[test]
    fn the_wanted_language_wins_between_identical_sources() {
        let mut query = query();
        query.name = Some("MangaDex".into());
        query.lang = Some(Language::from_code("ru").unwrap());

        let matches = loaded().search(&query).unwrap();
        let both: Vec<_> = matches
            .iter()
            .filter(|m| m.extension.name == "MangaDex")
            .collect();
        assert_eq!(both.len(), 2, "the English one is still offered");
        assert_eq!(both[0].extension.lang, Language::from_code("ru").unwrap());
        assert!(both[1].score < auto_bar(), "not unattended, though");
    }

    #[test]
    fn an_unrelated_name_is_not_a_candidate() {
        let mut query = query();
        query.name = Some("Bato.to".into());

        assert!(loaded().search(&query).unwrap().is_empty());
    }

    #[test]
    fn an_empty_query_offers_everything_and_settles_nothing() {
        let matches = loaded().search(&query()).unwrap();

        assert_eq!(matches.len(), 3);
        assert!(matches.iter().all(|m| m.score < auto_bar()));
    }

    #[test]
    fn another_app_gets_nothing() {
        let repo = loaded();
        let matches = repo.search(&ExtensionQuery::new(converter_core::ReaderID("aidoku")));

        assert!(matches.unwrap().is_empty());
    }

    #[test]
    fn searching_before_a_refresh_is_empty_rather_than_wrong() {
        assert!(Keiyoushi::new().search(&query()).unwrap().is_empty());
    }

    #[test]
    fn a_broken_index_names_itself() {
        let error = Keiyoushi::new().load("{ \"extensionList\": ").unwrap_err();

        assert!(
            matches!(error, ExtensionError::MalformedIndex { repo, .. } if repo == "Keiyoushi")
        );
    }

    #[test]
    fn a_refresh_replaces_the_previous_index() {
        let mut repo = loaded();
        repo.load(
            r#"{ "extensionList": { "extensions": [ { "name": "Bato.to", "sources": [
                 { "id": "31", "name": "Bato.to", "language": "en",
                   "homeUrl": "https://bato.to" } ] } ] } }"#,
        )
        .unwrap();

        let names: Vec<&str> = repo
            .index
            .iter()
            .map(|s| s.extension.name.as_str())
            .collect();
        assert_eq!(names, ["Bato.to"], "the previous sources are gone");
        // ...and what the last index told us about the repo itself survives a
        // payload that doesn't repeat it.
        assert_eq!(repo.info().short_name.as_deref(), Some("KEI"));
    }

    /// An index we get nothing out of is a schema break, and has to read as one:
    /// left as an empty repository it would sail through `refresh` and take the
    /// user's whole library with it at `resolve` time.
    #[test]
    fn an_index_with_nothing_in_it_is_malformed() {
        for payload in [
            r#"{ "extensionList": { "extensions": [] } }"#,
            // `extensions` renamed out from under us.
            r#"{ "extensionList": { "extensionList": [] } }"#,
            // ...or `sources`, which `#[serde(default)]` would read as none.
            r#"{ "extensionList": { "extensions": [ { "name": "MangaDex",
                 "sourceList": [ { "id": "1", "name": "MangaDex",
                 "language": "en", "homeUrl": "https://mangadex.org" } ] } ] } }"#,
        ] {
            let error = Keiyoushi::new().load(payload).unwrap_err();
            assert!(
                matches!(error, ExtensionError::MalformedIndex { .. }),
                "{payload} gave {error}"
            );
        }
    }

    #[test]
    fn a_rejected_index_leaves_the_last_good_one_alone() {
        let mut repo = loaded();
        repo.load(r#"{ "name": "Nonsense", "extensionList": { "extensions": [] } }"#)
            .unwrap_err();

        assert_eq!(repo.search(&query()).unwrap().len(), 3);
        assert_eq!(repo.info().name.as_deref(), Some("Keiyoushi"));
    }

    /// `equivalent_of` carries the source app's id into the query, and small
    /// integer ids exist on both sides, so an id alone can't mean exact.
    #[test]
    fn an_id_that_collided_is_not_an_exact_match() {
        let mut query = query();
        // "9" is the ru MangaDex here; some other app's "9" is anything at all.
        query.external_id = Some("9".into());
        query.name = Some("Batoto".into());

        let matches = loaded().search(&query).unwrap();

        assert!(
            matches.iter().all(|m| m.score < 1.0),
            "a contradicting name should stop the id short of exact"
        );
    }

    #[test]
    fn an_id_still_matches_through_a_renamed_source() {
        let mut query = query();
        query.external_id = Some("9".into());
        query.name = Some("MangaDex (RU)".into());

        assert_eq!(best(&loaded(), &query).score, 1.0);
    }

    #[test]
    fn hosts_normalize_before_they_are_compared() {
        assert_eq!(
            host_of("https://WWW.MangaDex.org:443/titles?x=1"),
            host_of("mangadex.org")
        );
        assert_eq!(
            host_of("https://user:pw@mangadex.org/"),
            host_of("https://mangadex.org")
        );
        assert!(host_of("https://").is_none());
        assert!(same_site("mangadex.org", "mangadex.to"));
        assert!(same_site("api.comick.live", "comick.live"));
        assert!(!same_site("mangadex.org", "comick.live"));
        // `com.br` and friends are suffix, not site: these are six different
        // scanlation groups, not one wearing six tlds.
        assert!(!same_site("tsundoku.com.br", "cafecomyaoi.com.br"));
        assert!(!same_site("asurascans.com.tr", "holyscans.com.tr"));
        assert!(!same_site("alphapolis.co.jp", "book.dmm.co.jp"));
        assert!(!same_site("mangadex.co.uk", "comick.co.uk"));
        // ...while a site that really did move tlds still lines up.
        assert!(same_site("asurascans.com.tr", "asurascans.net"));
        assert!(same_site("leitor.kamisama.com.br", "kamisama.org"));
    }

    #[test]
    fn names_normalize_before_they_are_compared() {
        assert_eq!(name_key("Tachiyomi: MangaDex"), name_key("manga-dex"));
        assert_eq!(
            name_similarity(&name_key("MangaDex"), &name_key("mangadex")),
            1.0
        );
        assert!(name_similarity(&name_key("MangaDex"), &name_key("MangaPark")) < 0.8);
    }

    /// Against the real index, so a schema change surfaces here rather than as
    /// an empty repository. Ignored because it needs the network:
    /// `cargo test -p converter-tachi -- --ignored`.
    #[tokio::test]
    #[ignore = "hits the network"]
    async fn the_live_index_still_parses() {
        let mut repo = Keiyoushi::new();
        repo.refresh().await.expect("the live index should load");

        assert!(
            repo.index.len() > 1000,
            "only {} sources parsed",
            repo.index.len()
        );
        assert!(repo.info().signing_key.is_some());

        let mut query = query();
        query.name = Some("MangaDex".into());
        query.url = Some("https://mangadex.org".into());
        let best = best(&repo, &query);
        assert_eq!(best.extension.name, "MangaDex");
        assert!(best.score >= auto_bar());
    }
}
