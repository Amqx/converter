use crate::reader::ReaderId;
use std::fmt::{Display, Write as _};

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
        self.render_header(&mut out);
        self.render_repos(&mut out);
        self.render_summary(&mut out);
        self.render_mappings(&mut out);
        self.render_skipped(&mut out);
        out
    }

    fn render_header(&self, out: &mut String) {
        let _ = writeln!(out, "Manga backup conversion report");
        let _ = writeln!(out, "==============================");
        let _ = writeln!(out, "Date:    {}", self.timestamp);
        let _ = writeln!(out, "From:    {}  ({})", self.from, self.input_file);
        let _ = writeln!(out, "To:      {}  ({})", self.to, self.output_file);
        out.push('\n');
    }

    fn render_repos(&self, out: &mut String) {
        let _ = writeln!(out, "Source repositories consulted");
        let _ = writeln!(out, "-----------------------------");
        if self.repos.is_empty() {
            out.push_str("  None.\n");
        }
        for repo in &self.repos {
            if repo.name.is_empty() {
                let _ = writeln!(out, "  - {}: {}", repo.reader, repo.url);
            } else {
                let _ = writeln!(out, "  - {}: {} ({})", repo.reader, repo.name, repo.url);
            }
        }
        out.push('\n');
    }

    fn render_summary(&self, out: &mut String) {
        // Lost-impact totals from explicit skip/drop lists.
        let skipped = || self.write.skipped_sources.iter().flat_map(|s| &s.manga);
        let manga_skipped = skipped().count() + self.read.dropped.len();
        let chapters_skipped = skipped().map(|m| m.chapters).sum::<usize>()
            + self.read.dropped.iter().map(|m| m.chapters).sum::<usize>();
        let history_skipped = skipped().map(|m| m.history).sum::<usize>();

        let _ = writeln!(out, "Summary");
        let _ = writeln!(out, "-------");
        summary_row(out, "", "Found", "Converted", "Skipped");
        summary_row_counts(out, "Manga", self.write.manga, manga_skipped);
        summary_row_counts(out, "Chapters", self.write.chapters, chapters_skipped);
        summary_row_counts(out, "History", self.write.history, history_skipped);
        // Categories are never skipped.
        summary_row(
            out,
            "Categories",
            self.write.categories,
            self.write.categories,
            "-",
        );
        out.push('\n');
    }

    fn render_mappings(&self, out: &mut String) {
        let _ = writeln!(out, "Source mapping");
        let _ = writeln!(out, "--------------");
        if self.write.mappings.is_empty() {
            out.push_str("  None.\n");
        }
        for mapping in &self.write.mappings {
            let _ = writeln!(out, "  {}  ->  {}", mapping.from, mapping.to);
        }
        out.push('\n');
    }

    fn render_skipped(&self, out: &mut String) {
        let _ = writeln!(out, "Skipped entries");
        let _ = writeln!(out, "---------------");
        if self.write.skipped_sources.is_empty() && self.read.dropped.is_empty() {
            out.push_str("  None.\n");
            return;
        }
        for source in &self.write.skipped_sources {
            let _ = writeln!(out, "  Source \"{}\" — {}", source.source_label, source.reason);
            for manga in &source.manga {
                let _ = writeln!(
                    out,
                    "      - {}  (chapters: {}, history: {})",
                    manga.title, manga.chapters, manga.history
                );
            }
        }
        for manga in &self.read.dropped {
            let _ = writeln!(
                out,
                "  {} — {}\n      (chapters: {})",
                manga.title, manga.reason, manga.chapters
            );
        }
    }
}

fn summary_row(
    out: &mut String,
    label: &str,
    found: impl Display,
    converted: impl Display,
    skipped: impl Display,
) {
    let _ = writeln!(out, "  {label:<12}{found:>8}{converted:>12}{skipped:>10}");
}

fn summary_row_counts(out: &mut String, label: &str, converted: usize, skipped: usize) {
    summary_row(out, label, converted + skipped, converted, skipped);
}