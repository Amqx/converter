//! Shared scoring helpers for [`ExtensionRepository`](super::types::ExtensionRepository)
//! implementations.

use crate::extensions::lang::Language;

/// Lowercase alphanumerics only, minus the app prefix some repos put in front.
pub fn name_key(name: &str) -> String {
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
/// length of the longer one. Both sides are expected to have been through
/// [`name_key`] already.
pub fn name_similarity(a: &str, b: &str) -> f32 {
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

/// The bare host of a url, lowercased and stripped of scheme, credentials,
/// port, `www.` and trailing dot, so two spellings of one site compare equal.
pub fn host_of(url: &str) -> Option<String> {
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

/// Hosting platforms that hand every group a label under one shared domain, so
/// the domain identifies nobody and the label above it is the site. Matched on
/// the label alone rather than on the whole suffix, because the same platform
/// is served under many tlds - `blogspot.com`, `blogspot.co.id` and
/// `blogspot.com.br` are all Blogger, and a list of suffixes would have to
/// spell out every one of them.
const PLATFORM_LABELS: [&str; 13] = [
    "blogspot",
    "github",
    "gitlab",
    "wordpress",
    "tumblr",
    "pages",
    "vercel",
    "netlify",
    "neocities",
    "herokuapp",
    "onrender",
    "glitch",
    "web",
];

/// Whether two hosts look like the same site wearing a different tld - the
/// usual shape of a scanlator that moved between one backup and the next.
pub fn same_site(a: &str, b: &str) -> bool {
    fn site_label(host: &str) -> Option<&str> {
        // Walking from the tld inwards: each step below is part of the suffix
        // rather than part of the site, so the label after them is the site.
        let mut labels = host.rsplit('.');
        labels.next()?;
        let mut candidate = labels.next()?;

        if SECOND_LEVEL_SUFFIXES.contains(&candidate) {
            candidate = labels.next()?;
        }
        if PLATFORM_LABELS.contains(&candidate) {
            candidate = labels.next()?;
        }

        (candidate.len() >= 3).then_some(candidate)
    }

    matches!((site_label(a), site_label(b)), (Some(a), Some(b)) if a == b)
}

/// How well a language the candidate serves answers the one the entry wanted,
/// as a factor to scale a score by.
pub fn language_agreement(candidate: &Language, wanted: &Language) -> f32 {
    match (candidate, wanted) {
        (a, b) if a == b => 1.0,
        // No evidence either way, so language must not move the score at all - the same neutral
        // 1.0 a caller gets by leaving `ExtensionQuery::lang` unset. Anything less would tax
        // every candidate for a fact the source backup simply never recorded.
        (Language::Unknown, _) | (_, Language::Unknown) => 1.0,
        (Language::All, _) | (_, Language::All) => 0.95,
        (Language::Tagged { iso: a, .. }, Language::Tagged { iso: b, .. }) if a == b => 0.85,
        _ => 0.4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(host_of("").is_none());
    }

    #[test]
    fn a_site_that_moved_tld_still_lines_up() {
        assert!(same_site("mangadex.org", "mangadex.to"));
        assert!(same_site("api.comick.live", "comick.live"));
        assert!(same_site("asurascans.com.tr", "asurascans.net"));
        assert!(same_site("leitor.kamisama.com.br", "kamisama.org"));
        assert!(!same_site("mangadex.org", "comick.live"));
    }

    /// `com.br` and friends are suffix, not site: these are six different
    /// scanlation groups, not one wearing six tlds.
    #[test]
    fn a_shared_suffix_is_not_a_shared_site() {
        assert!(!same_site("tsundoku.com.br", "cafecomyaoi.com.br"));
        assert!(!same_site("asurascans.com.tr", "holyscans.com.tr"));
        assert!(!same_site("alphapolis.co.jp", "book.dmm.co.jp"));
        assert!(!same_site("mangadex.co.uk", "comick.co.uk"));
    }

    /// Platforms like Blogger and GitHub Pages are where unrelated scanlators
    /// happen to be hosted, not a site that moved.
    #[test]
    fn a_platform_suffix_is_not_a_shared_site() {
        assert!(!same_site(
            "komikcast.blogspot.com",
            "mangaindo.blogspot.com"
        ));
        assert!(!same_site("mangax.github.io", "mangay.github.io"));
        assert!(!same_site("scan-a.pages.dev", "scan-b.pages.dev"));
        assert!(!same_site("grupo1.neocities.org", "grupo2.neocities.org"));
        assert!(!same_site("a.vercel.app", "b.vercel.app"));
        assert!(!same_site("groupa.wordpress.com", "groupb.wordpress.com"));
        assert!(!same_site("groupa.tumblr.com", "groupb.tumblr.com"));
        assert!(!same_site("groupa.netlify.app", "groupb.netlify.app"));
        assert!(!same_site("groupa.herokuapp.com", "groupb.herokuapp.com"));
    }

    /// Blogger serves the same blog under a per-region domain, and the regional
    /// ones put a ccSLD between the platform and the tld. Reading only the two
    /// trailing labels stops at `blogspot`, which every group on the platform
    /// shares.
    #[test]
    fn a_platform_under_a_country_tld_is_still_a_platform() {
        assert!(!same_site(
            "komikcast.blogspot.co.id",
            "mangaindo.blogspot.co.id"
        ));
        assert!(!same_site(
            "komikcast.blogspot.com.br",
            "mangaindo.blogspot.com.br"
        ));
        assert!(!same_site("komikcast.blogspot.jp", "mangaindo.blogspot.jp"));
    }

    /// The bonus is only worth having if a genuine move still earns it.
    #[test]
    fn a_site_that_only_changed_tld_is_still_the_same_site() {
        assert!(same_site("mangadex.org", "mangadex.net"));
        assert!(same_site("asurascans.com", "asurascans.gg"));
        assert!(same_site("read.mangadex.org", "mangadex.net"));
    }

    #[test]
    fn names_normalize_before_they_are_compared() {
        assert_eq!(name_key("Tachiyomi: MangaDex"), name_key("manga-dex"));
        assert_eq!(name_key("Aidoku: MangaDex"), name_key("MangaDex"));
        assert_eq!(
            name_similarity(&name_key("MangaDex"), &name_key("mangadex")),
            1.0
        );
        assert!(name_similarity(&name_key("MangaDex"), &name_key("MangaPark")) < 0.8);
    }

    #[test]
    fn an_empty_name_matches_nothing_rather_than_everything() {
        assert_eq!(name_similarity("", "mangadex"), 0.0);
        assert_eq!(name_similarity("", ""), 1.0);
    }

    #[test]
    fn a_language_is_worth_most_to_itself() {
        let en = Language::from_code("en").unwrap();
        let pt = Language::from_code("pt").unwrap();
        let pt_br = Language::from_code("pt-BR").unwrap();

        assert_eq!(language_agreement(&en, &en), 1.0);
        assert!(language_agreement(&Language::All, &en) < 1.0);
        assert!(language_agreement(&pt_br, &pt) < language_agreement(&Language::All, &pt));
        assert!(language_agreement(&en, &pt) < language_agreement(&pt_br, &pt));
    }

    /// A reader whose format has no language field must not tax every candidate for it. `All`
    /// scores 0.95, which is enough to push a 0.75-ish candidate under `AutoBest`'s bar.
    #[test]
    fn an_unknown_language_costs_a_candidate_nothing() {
        let en = Language::from_code("en").unwrap();
        let pt_br = Language::from_code("pt-BR").unwrap();

        for known in [&en, &pt_br, &Language::All] {
            assert_eq!(language_agreement(&Language::Unknown, known), 1.0);
            assert_eq!(language_agreement(known, &Language::Unknown), 1.0);
        }
        assert!(
            language_agreement(&Language::All, &en) < language_agreement(&Language::Unknown, &en)
        );
    }
}
