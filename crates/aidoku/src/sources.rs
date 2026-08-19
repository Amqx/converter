use std::sync::Arc;
use std::time::Duration;

use converter_core::extensions::lang::Language;
use converter_core::extensions::matching::{
    host_of, language_agreement, name_key, name_similarity, same_site,
};
use converter_core::extensions::types::{
    Extension, ExtensionMatch, ExtensionQuery, ExtensionRepository, ExtensionRepositoryInfo,
    RefreshFuture,
};
use converter_core::extensions::{ExtResult, ExtensionError};
use serde::Deserialize;

use crate::Aidoku;

const MIN_SCORE: f32 = 0.35;
const MAX_INFERRED_SCORE: f32 = 0.95;
const NEUTRAL_SCORE: f32 = 0.5;
const URL_AGREEMENT: f32 = 0.6;
const URL_DISAGREEMENT: f32 = 0.85;
/// How alike the names have to look before we trust a matching id.
const ID_CORROBORATION: f32 = 0.4;

pub struct Amqx {
    info: ExtensionRepositoryInfo,
    index: Vec<IndexedSource>,
}

struct IndexedSource {
    extension: Arc<Extension>,
    name_keys: Vec<String>,
    host: Option<String>,
    langs: Vec<Language>,
}

impl Amqx {
    const URL: &str = "https://amqx.github.io/sources/index.min.json";
    const NAME: &str = "Amqx's Aidoku Sources";
    const TIMEOUT: Duration = Duration::from_secs(30);
    const USER_AGENT: &str = concat!("converter-aidoku/", env!("CARGO_PKG_VERSION"));

    pub fn new() -> Self {
        Self {
            info: ExtensionRepositoryInfo::new(
                Aidoku::ID,
                Some(Self::NAME.into()),
                Some("Amqx's Sources".into()),
                Some("https://amqx.github.io/sources/".into()),
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

    fn load(&mut self, json: &str) -> ExtResult<()> {
        let list: SourceList =
            serde_json::from_str(json).map_err(|error| ExtensionError::MalformedIndex {
                repo: Self::NAME.to_string(),
                message: error.to_string(),
            })?;

        let mut info = self.info.clone();
        info.name = non_empty(list.name).or_else(|| info.name.take());

        let sources: Vec<IndexedSource> = list
            .sources
            .into_iter()
            .filter_map(|source| Self::indexed(&info, source))
            .collect();

        if sources.is_empty() {
            return Err(ExtensionError::MalformedIndex {
                repo: Self::NAME.to_string(),
                message: "the list held no usable sources; the schema has probably moved"
                    .to_string(),
            });
        }

        self.info = info;
        self.index = sources;

        Ok(())
    }

    /// Returns `None` for a source we couldn't map onto.
    fn indexed(info: &ExtensionRepositoryInfo, source: ListSource) -> Option<IndexedSource> {
        let external_id = source.id.trim().to_string();
        if external_id.is_empty() {
            return None;
        }

        let name_keys: Vec<String> = std::iter::once(&source.name)
            .chain(source.alt_names.iter())
            .map(|name| name_key(name))
            .filter(|key| !key.is_empty())
            .collect();
        if name_keys.is_empty() {
            return None;
        }

        // A code we can't read is dropped rather than fatal
        let mut langs: Vec<Language> = Vec::new();
        for code in source.languages() {
            if let Ok(lang) = Language::from_code(code)
                && !langs.contains(&lang)
            {
                langs.push(lang);
            }
        }
        let lang = match langs.as_slice() {
            [] => return None,
            // Aidoku ships one source for every language it serves and switches
            // between them in its own settings, so a list of them is `All`
            // rather than whichever happens to come first.
            [only] => only.clone(),
            _ => Language::All,
        };

        Some(IndexedSource {
            host: host_of(&source.base_url),
            name_keys,
            langs,
            extension: Arc::new(Extension {
                reader: info.reader,
                name: source.name,
                external_id,
                url: source.base_url,
                lang,
                repo: info.clone(),
            }),
        })
    }
}

impl IndexedSource {
    /// How close the closest of the source's names comes to the wanted one.
    fn best_name_similarity(&self, wanted: &str) -> f32 {
        self.name_keys
            .iter()
            .map(|key| name_similarity(key, wanted))
            .fold(0.0_f32, f32::max)
    }

    /// The best any single language of the source's does against the wanted
    /// one: a multilingual source really does serve each of them, so it should
    /// not be marked down the way a lone `All` is.
    fn language_factor(&self, wanted: Option<&Language>) -> f32 {
        let Some(wanted) = wanted else { return 1.0 };

        self.langs
            .iter()
            .map(|lang| language_agreement(lang, wanted))
            .fold(0.0_f32, f32::max)
    }
}

impl Default for Amqx {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionRepository for Amqx {
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

    // Aidoku ids are `lang.site` strings, so only another Aidoku backup ever
    // hands us one that matches; the name still has to agree, because a repo
    // is free to hand the same id to a source it rewrote from scratch.
    if let Some(id) = &query.external_id
        && !id.is_empty()
        && *id == source.extension.external_id
        && wanted_name
            .as_deref()
            .is_none_or(|name| source.best_name_similarity(name) >= ID_CORROBORATION)
    {
        return 1.0;
    }

    let mut score = wanted_name
        .as_ref()
        .map_or(NEUTRAL_SCORE, |name| source.best_name_similarity(name));

    if let Some(host) = &wanted_host {
        score = match source.host.as_deref() {
            Some(known) if known == host => score + (1.0 - score) * URL_AGREEMENT,
            Some(known) if same_site(known, host) => score + (1.0 - score) * URL_AGREEMENT * 0.5,
            Some(_) => score * URL_DISAGREEMENT,
            None => score,
        };
    }

    score *= source.language_factor(query.lang.as_ref());

    score.clamp(0.0, MAX_INFERRED_SCORE)
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[derive(Debug, Deserialize)]
struct SourceList {
    name: Option<String>,
    sources: Vec<ListSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSource {
    id: String,
    name: String,
    /// Former names of the site, when the list bothers to record them.
    #[serde(default)]
    alt_names: Vec<String>,
    #[serde(default)]
    languages: Vec<String>,
    /// Lists written before Aidoku 0.7 spell a single `lang` here instead.
    #[serde(default)]
    lang: Option<String>,
    // Not `baseUrl`: the list capitalises the acronym, and serde's camelCase
    // would go looking for the wrong key.
    #[serde(default, rename = "baseURL")]
    base_url: String,
}

impl ListSource {
    fn languages(&self) -> impl Iterator<Item = &String> {
        self.languages.iter().chain(self.lang.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use converter_core::orchestrator::policy::AutoBest;

    const INDEX: &str = r#"{
      "name": "Amqx's Sources",
      "sources": [
        { "id": "multi.mangadex", "name": "MangaDex", "version": 3,
          "downloadURL": "sources/multi.mangadex-v3.aix",
          "languages": ["en", "ru", "pt-BR"], "contentRating": 0,
          "baseURL": "https://mangadex.org" },
        { "id": "en.asurascans", "name": "Asura Scans", "version": 18,
          "downloadURL": "sources/en.asurascans-v18.aix",
          "languages": ["en"], "contentRating": 0,
          "baseURL": "https://asurascans.com" },
        { "id": "en.violetscans", "name": "Violet Scans", "version": 1,
          "downloadURL": "sources/en.violetscans-v1.aix",
          "languages": ["en"], "altNames": ["Shojo Scans"], "contentRating": 0,
          "baseURL": "https://violetscans.com" },
        { "id": "", "name": "No Id", "languages": ["en"],
          "baseURL": "https://noid.test" },
        { "id": "xx.nowhere", "name": "Nowhere", "languages": ["klingon"],
          "baseURL": "https://nowhere.test" }
      ]
    }"#;

    fn loaded() -> Amqx {
        let mut repo = Amqx::new();
        repo.load(INDEX).expect("the fixture parses");
        repo
    }

    fn query() -> ExtensionQuery {
        ExtensionQuery::new(Aidoku::ID)
    }

    fn best(repo: &Amqx, query: &ExtensionQuery) -> ExtensionMatch {
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
        let matches = loaded().search(&query()).unwrap();
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn drops_sources_it_could_not_map_onto() {
        let repo = loaded();
        // An id-less source can't go in a backup, and a source we can't place
        // in any language would only ever be placed wrong.
        assert!(
            repo.index
                .iter()
                .all(|s| !s.extension.external_id.is_empty())
        );
        assert!(repo.index.iter().all(|s| s.extension.name != "Nowhere"));
    }

    #[test]
    fn repository_metadata_comes_from_the_list() {
        let info = loaded().info().clone();
        assert_eq!(info.reader, Aidoku::ID);
        assert_eq!(info.name.as_deref(), Some("Amqx's Sources"));
        // Nothing in an Aidoku list describes these, so ours have to survive it.
        assert_eq!(info.short_name.as_deref(), Some("Amqx's Sources"));
        assert_eq!(
            info.website.as_deref(),
            Some("https://amqx.github.io/sources/")
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
        query.external_id = Some("en.asurascans".into());

        let best = best(&loaded(), &query);
        assert_eq!(best.score, 1.0, "Strict only accepts 1.0");
        assert_eq!(best.extension.name, "Asura Scans");
    }

    #[test]
    fn only_an_id_reaches_one() {
        let mut query = query();
        query.name = Some("Asura Scans".into());
        query.url = Some("https://asurascans.com".into());
        query.lang = Some(Language::from_code("en").unwrap());

        let best = best(&loaded(), &query);
        assert!(
            best.score >= auto_bar(),
            "{} should clear the auto bar",
            best.score
        );
        assert!(best.score < 1.0, "an inferred match must not read as exact");
    }

    /// The shape every cross-app conversion takes: Tachiyomi's numeric id means
    /// nothing here, so the name and the site have to carry it.
    #[test]
    fn a_foreign_id_still_resolves_on_name_and_site() {
        let mut query = ExtensionQuery::new(Aidoku::ID);
        query.external_id = Some("2499283573021220255".into());
        query.name = Some("MangaDex".into());
        query.url = Some("https://mangadex.org".into());
        query.lang = Some(Language::from_code("ru").unwrap());

        let best = best(&loaded(), &query);
        assert_eq!(best.extension.external_id, "multi.mangadex");
        assert!(
            best.score >= auto_bar(),
            "{} should clear the auto bar",
            best.score
        );
    }

    /// Aidoku ships one source per site, Tachiyomi one per language: the
    /// language the user's entry had is one this source serves, and marking it
    /// down for being multilingual would push a fine match under the bar.
    #[test]
    fn a_multilingual_source_is_not_marked_down_for_a_language_it_serves() {
        let mut named = query();
        named.name = Some("MangaDex".into());

        let mut russian = named.clone();
        russian.lang = Some(Language::from_code("ru").unwrap());
        let mut unlisted = named.clone();
        unlisted.lang = Some(Language::from_code("ja").unwrap());

        let repo = loaded();
        assert_eq!(best(&repo, &russian).extension.lang, Language::All);
        assert_eq!(best(&repo, &russian).score, best(&repo, &named).score);
        assert!(best(&repo, &unlisted).score < best(&repo, &russian).score);
    }

    /// `pt-BR` and `pt` are different sources everywhere else, but one Aidoku
    /// source listing `pt-BR` still serves a `pt` entry better than nothing.
    #[test]
    fn a_near_language_lands_between_a_hit_and_a_miss() {
        let mut query = query();
        query.name = Some("MangaDex".into());
        query.lang = Some(Language::from_code("pt").unwrap());

        let best = best(&loaded(), &query);
        assert!(best.score < MAX_INFERRED_SCORE);
        assert!(
            best.score >= auto_bar(),
            "{} is still a good match",
            best.score
        );
    }

    #[test]
    fn a_former_name_matches_as_well_as_the_current_one() {
        let mut query = query();
        query.name = Some("Shojo Scans".into());

        let best = best(&loaded(), &query);
        assert_eq!(best.extension.name, "Violet Scans");
        assert!(
            best.score >= auto_bar(),
            "{} should clear the auto bar",
            best.score
        );
    }

    #[test]
    fn a_renamed_domain_still_matches_on_the_name() {
        let mut query = query();
        query.name = Some("Asura Scans".into());
        query.url = Some("https://asurascans.moved.example".into());

        let best = best(&loaded(), &query);
        assert!(
            best.score >= auto_bar(),
            "{} should still clear the bar",
            best.score
        );
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
        let matches = repo.search(&ExtensionQuery::new(converter_core::ReaderID("tachiyomi")));

        assert!(matches.unwrap().is_empty());
    }

    #[test]
    fn searching_before_a_refresh_is_empty_rather_than_wrong() {
        assert!(Amqx::new().search(&query()).unwrap().is_empty());
    }

    /// An id that came from another app is another app's numbering; a name that
    /// contradicts it says so.
    #[test]
    fn an_id_that_collided_is_not_an_exact_match() {
        let mut query = query();
        query.external_id = Some("en.asurascans".into());
        query.name = Some("MangaDex".into());

        let matches = loaded().search(&query).unwrap();

        assert!(
            matches.iter().all(|m| m.score < 1.0),
            "a contradicting name should stop the id short of exact"
        );
    }

    #[test]
    fn an_id_still_matches_through_a_renamed_source() {
        let mut query = query();
        query.external_id = Some("en.violetscans".into());
        query.name = Some("Violet Scans (Shojo)".into());

        assert_eq!(best(&loaded(), &query).score, 1.0);
    }

    /// Lists written before Aidoku 0.7 spell one `lang` where the current shape
    /// spells `languages`, and carry no site at all.
    #[test]
    fn an_older_list_shape_still_indexes() {
        let mut repo = Amqx::new();
        repo.load(r#"{ "sources": [ { "id": "en.guya", "name": "Guya", "lang": "en" } ] }"#)
            .unwrap();

        let source = &repo.index[0];
        assert_eq!(source.extension.lang, Language::from_code("en").unwrap());
        assert!(source.host.is_none());

        let mut query = query();
        query.name = Some("Guya".into());
        // A site we can't compare against must not drag the name down.
        query.url = Some("https://guya.moe".into());
        assert!(best(&repo, &query).score >= auto_bar());
    }

    #[test]
    fn a_broken_list_names_itself() {
        let error = Amqx::new().load("{ \"sources\": ").unwrap_err();

        assert!(matches!(error, ExtensionError::MalformedIndex { repo, .. } if repo == Amqx::NAME));
    }

    /// A list we get nothing out of is a schema break, and has to read as one:
    /// left as an empty repository it would sail through `refresh` and take the
    /// user's whole library with it at `resolve` time.
    #[test]
    fn a_list_with_nothing_in_it_is_malformed() {
        for payload in [
            r#"{ "sources": [] }"#,
            // `sources` renamed out from under us.
            r#"{ "sourceList": [ { "id": "en.guya", "name": "Guya",
                 "languages": ["en"] } ] }"#,
            // ...or the languages moved, which `#[serde(default)]` would read
            // as a source that serves none.
            r#"{ "sources": [ { "id": "en.guya", "name": "Guya",
                 "languageList": ["en"], "baseURL": "https://guya.moe" } ] }"#,
        ] {
            let error = Amqx::new().load(payload).unwrap_err();
            assert!(
                matches!(error, ExtensionError::MalformedIndex { .. }),
                "{payload} gave {error}"
            );
        }
    }

    #[test]
    fn a_rejected_list_leaves_the_last_good_one_alone() {
        let mut repo = loaded();
        repo.load(r#"{ "name": "Nonsense", "sources": [] }"#)
            .unwrap_err();

        assert_eq!(repo.search(&query()).unwrap().len(), 3);
        assert_eq!(repo.info().name.as_deref(), Some("Amqx's Sources"));
    }

    #[test]
    fn a_refresh_replaces_the_previous_index() {
        let mut repo = loaded();
        repo.load(
            r#"{ "sources": [ { "id": "en.batoto", "name": "Bato.to",
                 "languages": ["en"], "baseURL": "https://bato.to" } ] }"#,
        )
        .unwrap();

        let names: Vec<&str> = repo
            .index
            .iter()
            .map(|s| s.extension.name.as_str())
            .collect();
        assert_eq!(names, ["Bato.to"], "the previous sources are gone");
        // ...and what the last list told us about the repo itself survives a
        // payload that doesn't repeat it.
        assert_eq!(repo.info().name.as_deref(), Some("Amqx's Sources"));
    }

    /// Against the real list, so a schema change surfaces here rather than as
    /// an empty repository. Ignored because it needs the network:
    /// `cargo test -p converter-aidoku -- --ignored`.
    #[tokio::test]
    #[ignore = "hits the network"]
    async fn the_live_list_still_parses() {
        let mut repo = Amqx::new();
        repo.refresh().await.expect("the live list should load");

        assert!(
            repo.index.len() > 50,
            "only {} sources parsed",
            repo.index.len()
        );

        let mut query = query();
        query.name = Some("MangaDex".into());
        query.url = Some("https://mangadex.org".into());
        let best = best(&repo, &query);
        assert_eq!(best.extension.external_id, "multi.mangadex");
        assert!(best.score >= auto_bar());
    }
}
