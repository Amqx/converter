//! Sources whose ids the two apps spell differently.
//!
//! Two apps' extensions for one site can agree on the site and still disagree
//! on what an entry's id is. Tachiyomi's `MangaDex` keeps `/manga/{uuid}` in
//! `BackupManga.url` where Aidoku's keeps the bare uuid as `Manga.key`, and
//! Kagane's chapters are `/series/{series}/reader/{book}` on one side and the
//! book id alone on the other. Nothing in either extension index says so, so a
//! resolved pair is not enough on its own: the writer would put an id in the
//! backup that the Aidoku source cannot open, and the entry restores into a
//! library row that leads nowhere.
//!
//! [`BUILTIN`] is the table of those sources. A rule says what the other app's
//! spelling looks like and what to write instead, as a pattern with `{name}`
//! holes in it:
//!
//! ```ignore
//! from "tachiyomi" {
//!     "multi.mangadex" {
//!         manga: "/manga/{id}" => "{id}",
//!         chapter: "/chapter/{id}" => "{id}",
//!     }
//! }
//! ```
//!
//! A hole holds at least one character and ends at the text after it, so
//! `/series/{series}/reader/{book}` tells the two ids apart, and the pattern
//! has to match the whole id rather than the front of it - a hole at the end
//! of a pattern takes everything that is left, separators and all. Both fields
//! are
//! optional and `manga` comes first when both are there; a field left out is
//! written the way the backup spelled it. Where an app has changed its own
//! spelling over the years, give the field a list and the first pattern that
//! matches wins:
//!
//! ```ignore
//! manga: ["/manga/{id}", "/title/{id}"] => "{id}",
//! ```
//!
//! A rule only applies to backups read from the app it is declared under, so
//! the ids in an Aidoku-to-Aidoku conversion - already in Aidoku's spelling -
//! are left alone.
//!
//! An id no pattern matches is one nothing here knows how to translate, and
//! writing it unchanged is the broken library row the rule exists to prevent.
//! The writer drops those rows and reports them, so the pattern that was
//! missing turns up in the run rather than in the app a week later.

use std::collections::HashMap;

use converter_core::ReaderID;

/// [`BUILTIN`]'s spelling. See the module docs; `manga` comes before
/// `chapter`, both are optional, and either takes one pattern or a list of
/// them.
macro_rules! quirks {
    (
        $(
            from $from:literal {
                $(
                    $source:literal {
                        $(manga: $manga_from:tt => $manga_into:literal,)?
                        $(chapter: $chapter_from:tt => $chapter_into:literal,)?
                    }
                )*
            }
        )*
    ) => {
        &[$($(
            $crate::quirks::SourceQuirk {
                from: ::converter_core::ReaderID($from),
                source: $source,
                manga: quirks!(@rewrite $($manga_from => $manga_into)?),
                chapter: quirks!(@rewrite $($chapter_from => $chapter_into)?),
            }
        ),*),*]
    };

    (@rewrite) => { None };
    (@rewrite $from:tt => $into:literal) => {
        Some($crate::quirks::Rewrite {
            from: quirks!(@patterns $from),
            into: $into,
        })
    };

    (@patterns [$($pattern:literal),* $(,)?]) => { &[$($pattern),*] };
    (@patterns $pattern:literal) => { &[$pattern] };
}

/// The sources whose ids need translating, and how. A typo in a pattern here
/// is caught by `every_builtin_rule_parses` rather than at conversion time.
pub const BUILTIN: &[SourceQuirk] = quirks! {
    from "tachiyomi" {
        // `SeriesDto.toSManga` keeps writing `/series/$slug` "for
        // compatibility with existing bookmarks" long after the site moved to
        // `/comics/`, and `ChapterDto.toSChapter` appends `/chapter/$number`.
        // Aidoku's `helpers::get_manga_key` keeps only the slug and
        // `get_chapter_key` only the number. Both sides already drop the
        // eight-character random suffix the site puts on a series slug -
        // Tachiyomi's `URL_RANDOM_PART`, Aidoku's `rfind('-')` - so the slug
        // itself needs no further work. The `/manga/` form predates all of
        // that and names the same slug.
        "en.asurascans" {
            manga: ["/series/{slug}", "/manga/{slug}/"] => "{slug}",
            chapter: "/series/{slug}/chapter/{number}" => "{number}",
        }

        // `ChapterDto.toSChapter` writes `"$slug/$id"` so `pageListRequest`
        // can split the manga back out of it; Aidoku's `into_chapter` is
        // handed the manga key separately and stores the chapter id alone.
        // The entry id is the same document id on both sides.
        "en.atsumaru" {
            chapter: "{manga}/{chapter}" => "{chapter}",
        }

        // Comix's ids are the leading segment of a slug: `getMangaUrl` builds
        // `$baseUrl/title$url` out of `/{hid}-{title}` and `getChapterUrl`
        // `$baseUrl/$url` out of `title/{hid}-{title}/{id}-chapter-{number}`,
        // while Aidoku keys on `ComixManga.hid` and `ComixChapter.id` - the
        // same two ids, without the title that follows them.
        "en.comix" {
            manga: "/{id}-{slug}" => "{id}",
            chapter: [
                "title/{series}-{slug}/{id}-chapter-{number}",
                // A chapter the site named nothing after its id.
                "title/{series}-{slug}/{id}",
            ] => "{id}",
        }

        // Iken's `MangaDto.toSManga` writes `"$slug#$id"` and
        // `ChapterDto.toSChapter` `"/series/$seriesSlug/$slug#$id"`, carrying
        // the numeric ids in the fragment because the slugs are what the site
        // shows. Aidoku's `parse_basic_manga` and `IkenChapter::parse_chapter`
        // key on those ids alone, since `/api/post?postId=` and
        // `/api/chapter?chapterId=` are what the api takes.
        "en.hivescans" {
            manga: "{slug}#{id}" => "{id}",
            chapter: "/series/{series}/{slug}#{id}" => "{id}",
        }

        "en.magusmanga" {
            manga: "{slug}#{id}" => "{id}",
            chapter: "/series/{series}/{slug}#{id}" => "{id}",
        }

        // The mangabox sources below key on a site-relative path, which is
        // what Tachiyomi's `setUrlWithoutDomain` stores too - until the site
        // moved and the old host stayed in the library. Aidoku's `MangaBox`
        // pastes `params.base_url` in front of the key, so an absolute id
        // becomes two urls end to end. Stripping the current host fixes the
        // rows that moved with it; the ids left over name predecessor sites
        // whose slugs the new one does not serve, and those are dropped.
        "en.mangabat" {
            chapter: [
                "https://www.mangabats.com/manga/{slug}/{chapter}",
                "/manga/{slug}/{chapter}",
            ] => "/manga/{slug}/{chapter}",
        }

        "en.mangakakalot" {
            manga: "/manga/{slug}" => "/manga/{slug}",
            chapter: [
                "https://www.mangakakalot.gg/manga/{slug}/{chapter}",
                "/manga/{slug}/{chapter}",
            ] => "/manga/{slug}/{chapter}",
        }

        "en.manganato" {
            manga: "/manga/{slug}" => "/manga/{slug}",
            chapter: "/manga/{slug}/{chapter}" => "/manga/{slug}/{chapter}",
        }

        // E-Hentai is the one source here whose key is a whole url:
        // `EHGalleryItem::into_basic_manga` stores what `get_base_url` built,
        // and `rewrite_domain` swaps the host for whichever of the two sites
        // the reader is set to - so the gallery id and token are the part
        // that matters and `e-hentai.org` is only a spelling. Tachiyomi keeps
        // the path with the `?nw=always` that skips the content warning.
        // ExHentai shares both ids, and one rule covers it because rules are
        // keyed by the Aidoku source the two of them resolve to.
        "multi.ehentai" {
            manga: [
                "/g/{gid}/{token}/?nw=always",
                "/g/{gid}/{token}/",
            ] => "https://e-hentai.org/g/{gid}/{token}/",
            chapter: [
                "/g/{gid}/{token}/?nw=always",
                "/g/{gid}/{token}/",
            ] => "https://e-hentai.org/g/{gid}/{token}/",
        }

        // Kagane agrees on the entry as it stands - `SeriesDto.toSManga`
        // stores the series id on its own - but `BookDto.toSChapter` stores
        // the reader's path, and Aidoku's chapter key is the book id inside
        // it. Both fields have been respelled twice since September 2025 and
        // the app never rewrote what was already in the library, so a backup
        // of any age carries all three: the semicolon forms are what
        // `getPageList` now refuses as "Outdated chapter URL".
        "multi.kagane" {
            manga: ["/series/{series}", "{series}"] => "{series}",
            chapter: [
                "/series/{series}/reader/{book}",
                // `"$seriesId;$id;$pagesCount"`, and this one first: the pair
                // below would otherwise take `id;pages` for the book.
                "{series};{book};{pages}",
                "{series};{book}",
            ] => "{book}",
        }

        // `MangaDexHelper` builds both out of the API's uuids: `/manga/{id}`
        // for an entry, `/chapter/{id}` for a chapter. Aidoku's
        // `multi.mangadex` keys on the uuids themselves.
        "multi.mangadex" {
            manga: "/manga/{id}" => "{id}",
            chapter: "/chapter/{id}" => "{id}",
        }

        // Aidoku keys on the api's own two ids - `hid` for a title, the
        // numeric `id` for a chapter - and `get_manga_update` refuses a key
        // starting `/manga` outright ("Migrate this title to update
        // details."). Tachiyomi spells the title `/title/$hid-$slug` and the
        // chapter `$mangaUrl/$id-chapter-$number-$lang`; the `/manga/` and
        // `/read/` forms are older and the app never rewrote them, so one
        // library holds all three. `/read/` is the one that carries no
        // chapter id at all, and those rows are dropped.
        "multi.mangafire" {
            manga: ["/manga/{slug}.{id}", "/title/{id}-{slug}"] => "{id}",
            chapter: [
                "/manga/{slug}.{hid}/{id}-chapter-{number}",
                "/title/{hid}-{slug}/{id}-chapter-{number}",
            ] => "{id}",
        }

        // Both of nhentai's keys are the gallery id: Aidoku's `Gallery`
        // conversion stores `value.id` and rebuilds `/g/{id}` when it needs
        // the url, where Tachiyomi keeps the path it read.
        "multi.nhentai" {
            manga: "/g/{id}/" => "{id}",
            chapter: "/g/{id}/" => "{id}",
        }
    }
};

/// One Aidoku source's rules, as written in [`BUILTIN`].
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SourceQuirk {
    /// The app whose spelling the patterns describe, by
    /// [`Backup::reader`](converter_core::ir::types::Backup::reader).
    pub from: ReaderID,
    /// The Aidoku source, by the id its `Extension::external_id` carries.
    pub source: &'static str,
    pub manga: Option<Rewrite>,
    pub chapter: Option<Rewrite>,
}

/// Patterns to recognise the other app's spelling by, and the one to write.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Rewrite {
    /// Tried in order; the first that matches wins.
    pub from: &'static [&'static str],
    pub into: &'static str,
}

/// The table for one conversion: the rules the backup's own app declared, with
/// their patterns parsed.
pub struct Quirks {
    rules: HashMap<&'static str, SourceRules>,
    /// Sources whose rule would not parse. Kept, so the writer can refuse them
    /// outright rather than fall back to the untranslated ids the rule was
    /// there to replace.
    broken: HashMap<&'static str, String>,
}

pub struct SourceRules {
    manga: Option<Compiled>,
    chapter: Option<Compiled>,
}

/// What a rewrite did to one id.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Rewritten {
    /// No rule for this field: the backup's own spelling stands.
    Unchanged,
    Into(String),
    /// A rule applies and none of its patterns matched, so what this id would
    /// be in the target app is exactly what nothing here knows.
    NoMatch,
}

impl Quirks {
    pub fn compile(table: &'static [SourceQuirk], from: ReaderID) -> Self {
        let mut rules = HashMap::new();
        let mut broken = HashMap::new();

        for quirk in table.iter().filter(|quirk| quirk.from == from) {
            match SourceRules::compile(quirk) {
                Ok(compiled) => {
                    rules.insert(quirk.source, compiled);
                }
                Err(reason) => {
                    broken.insert(quirk.source, reason);
                }
            }
        }

        Self { rules, broken }
    }

    pub fn get(&self, source: &str) -> Option<&SourceRules> {
        self.rules.get(source)
    }

    /// Why this source's rule would not parse, for the writer to report.
    pub fn broken(&self, source: &str) -> Option<&str> {
        self.broken.get(source).map(String::as_str)
    }
}

impl SourceRules {
    fn compile(quirk: &SourceQuirk) -> Result<Self, String> {
        Ok(Self {
            manga: field("manga", quirk.manga.as_ref())?,
            chapter: field("chapter", quirk.chapter.as_ref())?,
        })
    }

    pub fn manga_id(&self, id: &str) -> Rewritten {
        apply(self.manga.as_ref(), id)
    }

    pub fn chapter_id(&self, id: &str) -> Rewritten {
        apply(self.chapter.as_ref(), id)
    }
}

fn field(name: &str, rewrite: Option<&Rewrite>) -> Result<Option<Compiled>, String> {
    rewrite
        .map(|rewrite| {
            Compiled::new(rewrite.from, rewrite.into).map_err(|reason| format!("{name}: {reason}"))
        })
        .transpose()
}

fn apply(compiled: Option<&Compiled>, id: &str) -> Rewritten {
    let Some(compiled) = compiled else {
        return Rewritten::Unchanged;
    };
    compiled
        .apply(id)
        .map_or(Rewritten::NoMatch, Rewritten::Into)
}

/// One field's patterns, parsed.
#[derive(Debug)]
struct Compiled {
    from: Vec<Vec<Part>>,
    into: Vec<Part>,
}

impl Compiled {
    fn new(patterns: &[&'static str], template: &'static str) -> Result<Self, String> {
        if patterns.is_empty() {
            return Err("no pattern to match".to_string());
        }

        let from: Vec<Vec<Part>> = patterns
            .iter()
            .copied()
            .map(parse)
            .collect::<Result<_, _>>()?;
        let into = parse(template)?;

        // Any one of the patterns can be the one that matched, so a hole the
        // output asks for has to be one all of them fill.
        for name in into.iter().filter_map(Part::hole) {
            let missing = from
                .iter()
                .zip(patterns)
                .find(|(parts, _)| !parts.iter().filter_map(Part::hole).any(|held| held == name));
            if let Some((_, pattern)) = missing {
                return Err(format!(
                    "`{template}` asks for `{{{name}}}`, which `{pattern}` never captures"
                ));
            }
        }

        Ok(Self { from, into })
    }

    fn apply(&self, id: &str) -> Option<String> {
        let held = self.from.iter().find_map(|pattern| capture(pattern, id))?;

        let mut out = String::with_capacity(id.len());
        for part in &self.into {
            match part {
                Part::Literal(literal) => out.push_str(literal),
                // `new` checked every pattern fills this one.
                Part::Hole(name) => out.push_str(
                    held.iter()
                        .find(|(hole, _)| hole == name)
                        .map_or("", |(_, held)| *held),
                ),
            }
        }

        Some(out)
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Part {
    Literal(&'static str),
    Hole(&'static str),
}

impl Part {
    const fn hole(&self) -> Option<&'static str> {
        match self {
            Self::Literal(_) => None,
            Self::Hole(name) => Some(*name),
        }
    }
}

/// Splits a pattern into the holes and the text between them.
fn parse(pattern: &'static str) -> Result<Vec<Part>, String> {
    let mut parts: Vec<Part> = Vec::new();
    let mut rest = pattern;

    while !rest.is_empty() {
        let Some(open) = rest.find('{') else {
            parts.push(Part::Literal(rest));
            break;
        };

        if let Some(literal) = rest.get(..open).filter(|text| !text.is_empty()) {
            parts.push(Part::Literal(literal));
        }

        let hole = rest.get(open..).unwrap_or_default();
        let Some(close) = hole.find('}') else {
            return Err(format!("`{pattern}` opens a hole it never closes"));
        };

        let name = hole.get(1..close).unwrap_or_default();
        if name.is_empty() {
            return Err(format!("`{pattern}` has a hole with no name"));
        }
        // Nothing between two holes says where the first one ends.
        if parts.last().is_some_and(|part| part.hole().is_some()) {
            return Err(format!(
                "`{pattern}` runs `{{{name}}}` straight into the hole before it"
            ));
        }

        parts.push(Part::Hole(name));
        rest = hole.get(close + 1..).unwrap_or_default();
    }

    if parts.is_empty() {
        return Err("an empty pattern matches nothing".to_string());
    }

    Ok(parts)
}

/// What each hole in `parts` holds when the whole of `id` matches it, or `None`
/// when it does not. A hole ends at the first occurrence of the text after it,
/// so `{series}` in `/series/{series}/reader/{book}` stops at the reader path
/// rather than swallowing it.
fn capture<'a>(parts: &[Part], id: &'a str) -> Option<Vec<(&'static str, &'a str)>> {
    let mut held: Vec<(&'static str, &'a str)> = Vec::new();
    let mut rest = id;
    let mut index = 0;

    while let Some(part) = parts.get(index) {
        match *part {
            Part::Literal(literal) => {
                rest = rest.strip_prefix(literal)?;
                index += 1;
            }
            // `parse` rejects two holes in a row, so whatever follows this one
            // is the text it ends at.
            Part::Hole(name) => {
                if let Some(&Part::Literal(next)) = parts.get(index + 1) {
                    // A hole holds something: an id with the piece missing is
                    // not an id this pattern describes.
                    let at = rest.find(next).filter(|at| *at > 0)?;
                    held.push((name, rest.get(..at)?));
                    rest = rest.get(at + next.len()..)?;
                    index += 2;
                } else {
                    if rest.is_empty() {
                        return None;
                    }
                    held.push((name, rest));
                    rest = "";
                    index += 1;
                }
            }
        }
    }

    // Anchored at both ends: a pattern describes a whole id, not the front of one.
    rest.is_empty().then_some(held)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TACHI: ReaderID = ReaderID("tachiyomi");

    fn compiled(from: &[&'static str], into: &'static str) -> Compiled {
        Compiled::new(from, into).expect("the pattern should parse")
    }

    fn applied(from: &'static str, into: &'static str, id: &str) -> Option<String> {
        compiled(&[from], into).apply(id)
    }

    /// The table is const data, so a typo in it is only a typo until something
    /// parses it. This is the something.
    #[test]
    fn every_builtin_rule_parses() {
        for quirk in BUILTIN {
            SourceRules::compile(quirk)
                .unwrap_or_else(|reason| panic!("{}: {reason}", quirk.source));
        }
    }

    #[test]
    fn mangadex_ids_lose_the_path_tachiyomi_keeps() {
        let quirks = Quirks::compile(BUILTIN, TACHI);
        let rules = quirks.get("multi.mangadex").expect("a rule for MangaDex");

        let uuid = "b41b1e9d-b60c-4b17-bfc2-93a9e0b2f0b8";
        assert_eq!(
            rules.manga_id(&format!("/manga/{uuid}")),
            Rewritten::Into(uuid.to_string())
        );
        assert_eq!(
            rules.chapter_id(&format!("/chapter/{uuid}")),
            Rewritten::Into(uuid.to_string())
        );
    }

    /// Kagane has respelled both fields twice and rewrote nothing already in
    /// the library, so one backup carries every generation of them at once.
    #[test]
    fn kagane_ids_of_every_generation_reach_the_same_key() {
        let quirks = Quirks::compile(BUILTIN, TACHI);
        let rules = quirks.get("multi.kagane").expect("a rule for Kagane");

        let series = |id: &str| rules.manga_id(id);
        assert_eq!(series("01JQ8"), Rewritten::Into("01JQ8".to_string()));
        assert_eq!(
            series("/series/01JQ8"),
            Rewritten::Into("01JQ8".to_string())
        );

        let book = |id: &str| rules.chapter_id(id);
        let expected = Rewritten::Into("01JQ9".to_string());
        assert_eq!(book("/series/01JQ8/reader/01JQ9"), expected);
        // The page count on the end is not part of the key, and neither is the
        // series in front of it.
        assert_eq!(book("01JQ8;01JQ9;78"), expected);
        assert_eq!(book("01JQ8;01JQ9"), expected);
    }

    /// Iken carries the ids Aidoku wants in the fragment of a slug, and both
    /// fields spell it the same way.
    #[test]
    fn iken_ids_come_out_of_the_fragment() {
        let quirks = Quirks::compile(BUILTIN, TACHI);

        for source in ["en.hivescans", "en.magusmanga"] {
            let rules = quirks.get(source).expect("a rule for an Iken source");
            assert_eq!(
                rules.manga_id("eleceed#12"),
                Rewritten::Into("12".to_string())
            );
            assert_eq!(
                rules.chapter_id("/series/eleceed/chapter-413#20565"),
                Rewritten::Into("20565".to_string())
            );
        }
    }

    /// The mangabox sources key on a path, and `MangaBox` pastes its own base
    /// url in front of it - so an id that kept the host has to lose it.
    #[test]
    fn a_mangabox_id_that_kept_its_host_loses_it_again() {
        let quirks = Quirks::compile(BUILTIN, TACHI);
        let rules = quirks
            .get("en.mangakakalot")
            .expect("a rule for MangaKakalot");

        let expected = Rewritten::Into("/manga/this-witch-of-mine/chapter-1".to_string());
        assert_eq!(
            rules.chapter_id("https://www.mangakakalot.gg/manga/this-witch-of-mine/chapter-1"),
            expected
        );
        assert_eq!(
            rules.chapter_id("/manga/this-witch-of-mine/chapter-1"),
            expected
        );

        // The predecessor sites' ids name slugs mangakakalot.gg does not
        // serve, so there is nothing to write them as.
        assert_eq!(
            rules.chapter_id("https://chapmanganato.com/manga-as956575/chapter-1"),
            Rewritten::NoMatch
        );
        assert_eq!(
            rules.manga_id("/read-ax0zw158504916009"),
            Rewritten::NoMatch
        );
    }

    /// E-Hentai's key is a whole url, so this rule builds one rather than
    /// stripping one. `ExHentai` resolves to the same Aidoku source and shares
    /// the gallery ids, which is why one rule covers both.
    #[test]
    fn ehentai_ids_become_the_url_aidoku_keys_on() {
        let quirks = Quirks::compile(BUILTIN, TACHI);
        let rules = quirks.get("multi.ehentai").expect("a rule for E-Hentai");

        let expected = Rewritten::Into("https://e-hentai.org/g/3611504/4f60ad8178/".to_string());
        assert_eq!(rules.manga_id("/g/3611504/4f60ad8178/?nw=always"), expected);
        assert_eq!(rules.chapter_id("/g/3611504/4f60ad8178/"), expected);
    }

    /// `MangaFire` has respelled the entry twice and the chapter once, and only
    /// the newer chapter spelling carries the id Aidoku keys on.
    #[test]
    fn mangafire_keeps_the_ids_its_api_takes() {
        let quirks = Quirks::compile(BUILTIN, TACHI);
        let rules = quirks.get("multi.mangafire").expect("a rule for MangaFire");

        let hid = Rewritten::Into("yko70".to_string());
        assert_eq!(rules.manga_id("/manga/a-stepmothers-marchenn.yko70"), hid);
        assert_eq!(rules.manga_id("/title/yko70-a-stepmothers-marchenn"), hid);

        assert_eq!(
            rules.chapter_id("/manga/a-stepmothers-marchenn.yko70/5910205-chapter-77.5-en"),
            Rewritten::Into("5910205".to_string())
        );
        // The `/read/` spelling names the chapter by number, and the number
        // is not the id.
        assert_eq!(
            rules.chapter_id("/read/daemabeopsaui-ttall.4j8jx/en/chapter-0"),
            Rewritten::NoMatch
        );
    }

    /// Asura kept `/series/` in the backup after the site moved to `/comics/`,
    /// and the chapter is numbered rather than slugged.
    #[test]
    fn asura_ids_lose_the_path_around_them() {
        let quirks = Quirks::compile(BUILTIN, TACHI);
        let rules = quirks.get("en.asurascans").expect("a rule for Asura Scans");

        let slug = Rewritten::Into("nano-machine".to_string());
        assert_eq!(rules.manga_id("/series/nano-machine"), slug);
        assert_eq!(rules.manga_id("/manga/nano-machine/"), slug);
        assert_eq!(
            rules.chapter_id("/series/nano-machine/chapter/283"),
            Rewritten::Into("283".to_string())
        );
    }

    /// The shape a rule was written for is the only one it claims to know.
    #[test]
    fn an_id_of_another_shape_is_not_rewritten_into_a_guess() {
        let quirks = Quirks::compile(BUILTIN, TACHI);
        let rules = quirks.get("multi.mangadex").expect("a rule for MangaDex");

        assert_eq!(rules.manga_id("/title/abc"), Rewritten::NoMatch);
        assert_eq!(rules.manga_id("/manga/"), Rewritten::NoMatch);
        assert_eq!(rules.manga_id("abc"), Rewritten::NoMatch);
    }

    /// An Aidoku backup's ids are already Aidoku's, and a rule written against
    /// Tachiyomi's spelling would call every one of them unrecognised.
    #[test]
    fn rules_only_apply_to_the_app_they_were_written_for() {
        let quirks = Quirks::compile(BUILTIN, ReaderID("aidoku"));

        assert!(quirks.get("multi.mangadex").is_none());
        assert!(quirks.broken("multi.mangadex").is_none());
    }

    #[test]
    fn a_hole_ends_at_the_text_after_it() {
        assert_eq!(
            applied(
                "/series/{series}/reader/{book}",
                "{series}",
                "/series/a/reader/b"
            ),
            Some("a".to_string())
        );
        // The first occurrence ends the hole, so a separator further along
        // belongs to the hole that reaches the end.
        assert_eq!(
            applied("/a/{id}", "{id}", "/a/b/c"),
            Some("b/c".to_string())
        );
    }

    #[test]
    fn an_output_can_rebuild_an_id_as_well_as_strip_one() {
        assert_eq!(
            applied("{id}", "/manga/{id}", "abc"),
            Some("/manga/abc".to_string())
        );
        assert_eq!(
            applied("/a/{one}/b/{two}", "{two}-{one}", "/a/1/b/2"),
            Some("2-1".to_string())
        );
    }

    #[test]
    fn later_patterns_cover_a_spelling_the_app_has_since_dropped() {
        let rewrite = compiled(&["/manga/{id}", "/title/{id}"], "{id}");

        assert_eq!(rewrite.apply("/manga/abc"), Some("abc".to_string()));
        assert_eq!(rewrite.apply("/title/abc"), Some("abc".to_string()));
        assert_eq!(rewrite.apply("/other/abc"), None);
    }

    #[test]
    fn a_pattern_that_cannot_mean_one_thing_is_refused() {
        let refused = |from: &[&'static str], into| {
            Compiled::new(from, into).expect_err("the pattern should be refused")
        };

        assert!(refused(&["{a}{b}"], "{a}").contains("straight into the hole before it"));
        assert!(refused(&["/a/{id"], "{id}").contains("never closes"));
        assert!(refused(&["/a/{}"], "x").contains("no name"));
        assert!(refused(&[], "{id}").contains("no pattern"));
        assert!(refused(&[""], "x").contains("empty pattern"));
        // The second pattern is as likely to be the one that matched.
        assert!(
            refused(&["/manga/{id}", "/title/{slug}"], "{id}")
                .contains("`/title/{slug}` never captures")
        );
    }

    /// A rule that will not parse is the one case where leaving the ids alone
    /// is not the safe answer: it is that source's own ids the rule is about.
    #[test]
    fn a_broken_rule_is_kept_as_a_failure_rather_than_dropped() {
        const TABLE: &[SourceQuirk] = quirks! {
            from "tachiyomi" {
                "en.broken" {
                    manga: "{a}{b}" => "{a}",
                }
            }
        };

        let quirks = Quirks::compile(TABLE, TACHI);

        assert!(quirks.get("en.broken").is_none());
        assert!(
            quirks
                .broken("en.broken")
                .is_some_and(|reason| reason.starts_with("manga:"))
        );
    }

    #[test]
    fn the_macro_takes_one_pattern_or_a_list_of_them() {
        const TABLE: &[SourceQuirk] = quirks! {
            from "tachiyomi" {
                "en.one" {
                    manga: "/a/{id}" => "{id}",
                }
                "en.many" {
                    manga: ["/a/{id}", "/b/{id}"] => "{id}",
                    chapter: "/c/{id}" => "{id}",
                }
            }
        };

        assert_eq!(TABLE.len(), 2);
        assert_eq!(TABLE[0].manga.as_ref().unwrap().from.to_vec(), ["/a/{id}"]);
        assert_eq!(TABLE[0].chapter, None);
        assert_eq!(
            TABLE[1].manga.as_ref().unwrap().from.to_vec(),
            ["/a/{id}", "/b/{id}"]
        );
        assert_eq!(TABLE[1].chapter.as_ref().unwrap().into, "{id}");
    }
}
