use crate::reader::ReaderId;

pub struct RepoInfo {
    pub reader: ReaderId,
    pub name: String,
    pub url: String,
}

#[derive(Default)]
pub struct ReadReport {
    pub dropped: Vec<DroppedManga>,
}

pub struct DroppedManga {
    pub title: String,
    pub reason: String,
    pub chapters: usize,
}

#[derive(Default)]
pub struct WriteReport {
    pub manga: usize,
    pub chapters: usize,
    pub history: usize,
    pub categories: usize,
    pub mappings: Vec<SourceMapping>,
    pub skipped_sources: Vec<SkippedSource>,
}

pub struct SourceMapping {
    pub from: String,
    pub to: String,
}

pub struct SkippedSource {
    pub source_label: String,
    pub reason: String,
    pub manga: Vec<SkippedManga>,
}

pub struct SkippedManga {
    pub title: String,
    pub chapters: usize,
    pub history: usize,
}

pub struct ConversionReport {
    pub timestamp: String,
    pub input_file: String,
    pub output_file: String,
    pub from: ReaderId,
    pub to: ReaderId,
    pub repos: Vec<RepoInfo>,
    pub read: ReadReport,
    pub write: WriteReport,
}

impl ConversionReport {
    pub fn render(&self) -> String {
        let mut out = String::new();

        out.push_str("Manga backup conversion report\n");
        out.push_str("==============================\n");
        out.push_str(&format!("Date:    {}\n", self.timestamp));
        out.push_str(&format!("From:    {}  ({})\n", self.from, self.input_file));
        out.push_str(&format!("To:      {}  ({})\n", self.to, self.output_file));
        out.push('\n');

        out.push_str("Source repositories consulted\n");
        out.push_str("-----------------------------\n");
        if self.repos.is_empty() {
            out.push_str("  None.\n");
        } else {
            for repo in &self.repos {
                if repo.name.is_empty() {
                    out.push_str(&format!("  - {}: {}\n", repo.reader, repo.url));
                } else {
                    out.push_str(&format!("  - {}: {} ({})\n", repo.reader, repo.name, repo.url));
                }
            }
        }
        out.push('\n');

        // Lost-impact totals from explicit skip/drop lists.
        let read_drop_manga = self.read.dropped.len();
        let read_drop_chapters: usize = self.read.dropped.iter().map(|m| m.chapters).sum();
        let skip_manga: usize = self.write.skipped_sources.iter().map(|s| s.manga.len()).sum();
        let skip_chapters: usize = self
            .write
            .skipped_sources
            .iter()
            .flat_map(|s| &s.manga)
            .map(|m| m.chapters)
            .sum();
        let skip_history: usize = self
            .write
            .skipped_sources
            .iter()
            .flat_map(|s| &s.manga)
            .map(|m| m.history)
            .sum();

        let manga_skipped = skip_manga + read_drop_manga;
        let chapters_skipped = skip_chapters + read_drop_chapters;

        out.push_str("Summary\n");
        out.push_str("-------\n");
        out.push_str(&format!(
            "  {:<12}{:>8}{:>12}{:>10}\n",
            "", "Found", "Converted", "Skipped"
        ));
        let found = self.write.manga + manga_skipped;
        let converted = self.write.manga;
        out.push_str(&format!(
            "  {:<12}{:>8}{:>12}{:>10}\n",
            "Manga", found, converted, manga_skipped
        ));
        let found = self.write.chapters + chapters_skipped;
        let converted = self.write.chapters;
        out.push_str(&format!(
            "  {:<12}{:>8}{:>12}{:>10}\n",
            "Chapters", found, converted, chapters_skipped
        ));
        let found = self.write.history + skip_history;
        let converted = self.write.history;
        out.push_str(&format!(
            "  {:<12}{:>8}{:>12}{:>10}\n",
            "History", found, converted, skip_history
        ));
        // Categories are never skipped.
        out.push_str(&format!(
            "  {:<12}{:>8}{:>12}{:>10}\n",
            "Categories", self.write.categories, self.write.categories, "-"
        ));
        out.push('\n');

        out.push_str("Source mapping\n");
        out.push_str("--------------\n");
        if self.write.mappings.is_empty() {
            out.push_str("  None.\n");
        } else {
            for mapping in &self.write.mappings {
                out.push_str(&format!("  {}  ->  {}\n", mapping.from, mapping.to));
            }
        }
        out.push('\n');

        out.push_str("Skipped entries\n");
        out.push_str("---------------\n");
        if self.write.skipped_sources.is_empty() && self.read.dropped.is_empty() {
            out.push_str("  None.\n");
        } else {
            for source in &self.write.skipped_sources {
                out.push_str(&format!(
                    "  Source \"{}\" — {}\n",
                    source.source_label, source.reason
                ));
                for manga in &source.manga {
                    out.push_str(&format!(
                        "      - {}  (chapters: {}, history: {})\n",
                        manga.title, manga.chapters, manga.history
                    ));
                }
            }
            for manga in &self.read.dropped {
                out.push_str(&format!(
                    "  {} — {}\n      (chapters: {})\n",
                    manga.title, manga.reason, manga.chapters
                ));
            }
        }
        out
    }
}