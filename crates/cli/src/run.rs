//! What each subcommand actually does.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use converter_aidoku::Aidoku;
use converter_core::extensions::types::Extension;
use converter_core::ir::types::Backup;
use converter_core::orchestrator::OrchestratorError;
use converter_core::orchestrator::policy::AutoBest;
use converter_tachi::Tachiyomi;

use crate::cli::{ConvertArgs, InspectArgs, PolicyKind};
use crate::formats::{self, Format};
use crate::report;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("could not read {path}: {source}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    WriteFile {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("unknown app '{name}'; known apps are {known}")]
    UnknownApp { name: String, known: String },
    #[error("could not tell which app '{path}' came from; name it with --from (one of {known})")]
    UndetectedApp { path: String, known: String },
    #[error("this build can only read {label} backups; it can write {writable}")]
    NotWritable { label: String, writable: String },
    #[error("--output is the input backup; converting would overwrite it")]
    OutputIsInput,
    #[error("{path} already exists; pass --force to overwrite it")]
    OutputExists { path: String },
    #[error("{path} does not exist, so there is nowhere to write the output")]
    OutputParentMissing { path: String },
    #[error(
        "every entry was dropped ({dropped} of {total}); nothing but categories would be written"
    )]
    AllEntriesDropped { dropped: usize, total: usize },
    #[error(transparent)]
    Pipeline(#[from] OrchestratorError),
}

type CliResult<T> = Result<T, CliError>;

/// read -> resolve -> prune -> write, narrating each stage.
pub async fn convert(args: ConvertArgs) -> CliResult<()> {
    let source = source_format(args.from.as_deref(), &args.input)?;
    let target = target_format(&args.to)?;

    // Settle where the output goes before reading anything: refusing to
    // overwrite is worth saying now rather than after a network refresh and a
    // whole conversion the user cannot keep.
    let output = args
        .output
        .unwrap_or_else(|| default_output(&args.input, &target));
    let same = same_file(&args.input, &output);
    let exists = output.exists();

    if same {
        if args.dry_run {
            println!(
                "warning: --output is the input backup; a real run would refuse to overwrite it"
            );
        } else {
            return Err(CliError::OutputIsInput);
        }
    }
    if exists && !same {
        if args.dry_run {
            println!(
                "warning: {} already exists; a real run would need --force to overwrite it",
                output.display()
            );
        } else if !args.force {
            return Err(CliError::OutputExists {
                path: output.display().to_string(),
            });
        }
    }
    if !args.dry_run
        && let Some(parent) = output_parent(&output)
        && !parent.exists()
    {
        return Err(CliError::OutputParentMissing {
            path: parent.display().to_string(),
        });
    }

    if args.policy == PolicyKind::Strict
        && (args.min_score - AutoBest::default().min_score).abs() > f32::EPSILON
    {
        eprintln!("warning: --min-score has no effect with --policy strict");
    }

    // Likewise, parse the backup before going near the network: one this build
    // cannot open should not cost an index refresh first.
    let bytes = read_file(&args.input)?;
    let mut orchestrator = converter_core::orchestrator! {
        policy: args.policy.with(args.min_score),
        formats: [Tachiyomi, Aidoku],
    };

    println!("{} -> {}", source.label, target.label);

    let (mut backup, read) = orchestrator.read(&bytes, source.id)?;
    report::read(&read);

    let refresh = orchestrator.refresh().await;
    report::refresh(&refresh, target.id);

    let mapping = orchestrator.resolve(&backup, target.id)?;
    report::mapping(&mapping, args.verbose);

    let prune = orchestrator.prune(&mut backup, &mapping);
    // An entry resolution never saw means the reader handed out a second
    // `Arc<Extension>` for a source already in the table; that is a bug in this
    // build, not a backup the target app has no counterpart for.
    if let Some(entry) = prune.unknown.first() {
        return Err(OrchestratorError::UnknownEntryExtension {
            entry: entry.title.clone(),
            extension: entry.extension.name.clone(),
        }
        .into());
    }
    report::prune(&prune, backup.entries.len());

    let total = backup.entries.len() + prune.dropped();
    if backup.entries.is_empty() && prune.dropped() > 0 {
        return Err(CliError::AllEntriesDropped {
            dropped: prune.dropped(),
            total,
        });
    }

    if args.dry_run {
        println!(
            "dry run: nothing written (would have been {})",
            output.display()
        );
        return Ok(());
    }

    let (encoded, write) = orchestrator.write(&backup, &mapping, target.id)?;
    report::write(&write);

    fs::write(&output, &encoded).map_err(|source| CliError::WriteFile {
        path: output.display().to_string(),
        source,
    })?;
    println!(
        "wrote {} ({})",
        output.display(),
        report::size(encoded.len())
    );

    Ok(())
}

/// Reads a backup and describes it. No repository is consulted, so this works
/// offline and says nothing about what the entries would convert into.
pub fn inspect(args: &InspectArgs) -> CliResult<()> {
    let source = source_format(args.from.as_deref(), &args.input)?;
    let bytes = read_file(&args.input)?;

    // `read` never asks the matcher anything, so the policy here is unused.
    let orchestrator = converter_core::orchestrator! {
        policy: AutoBest::default(),
        formats: [Tachiyomi, Aidoku],
    };

    let (backup, read) = orchestrator.read(&bytes, source.id)?;
    report::read(&read);

    println!("{}", source.label);
    if let Some(version) = &backup.version {
        println!("version: {version}");
    }
    println!("extensions: {}", backup.extensions.len());
    println!(
        "entries: {} ({} in the library)",
        backup.entries.len(),
        backup.entries.iter().filter(|e| e.favorite).count()
    );
    println!(
        "chapters: {}",
        backup
            .entries
            .iter()
            .map(|e| e.chapters.len())
            .sum::<usize>()
    );
    println!("categories: {}", backup.categories.len());

    if args.verbose {
        for (name, entries) in entries_per_extension(&backup) {
            println!("  {name}: {entries}");
        }
        for category in &backup.categories {
            println!("  category: {}", category.name);
        }
    }

    Ok(())
}

/// What this build can read, and what it can write.
pub fn formats() {
    let all = formats::all();
    let id_width = all.iter().map(|f| f.id.0.len()).max().unwrap_or(0);
    let label_width = all.iter().map(|f| f.label.len()).max().unwrap_or(0);

    for format in &all {
        let extensions: Vec<String> = format
            .file_extensions
            .iter()
            .map(|ext| format!(".{ext}"))
            .collect();
        let direction = if format.writable {
            "read, write"
        } else {
            "read only"
        };

        print!(
            "{:<id_width$}  {:<11}  {:<label_width$}  [{}]",
            format.id.0,
            direction,
            format.label,
            extensions.join(", ")
        );
        if format.aliases.is_empty() {
            println!();
        } else {
            println!("  also: {}", format.aliases.join(", "));
        }
    }
}

/// `--from` if it was given, the file name otherwise. Nothing sniffs the bytes:
/// a protobuf decoder accepts far too much to guess with.
fn source_format(from: Option<&str>, input: &Path) -> CliResult<Format> {
    if let Some(name) = from {
        return formats::find(name).ok_or_else(|| CliError::UnknownApp {
            name: name.to_string(),
            known: formats::known_names().join(", "),
        });
    }

    formats::detect(input).ok_or_else(|| CliError::UndetectedApp {
        path: input.display().to_string(),
        known: formats::known_names().join(", "),
    })
}

fn target_format(to: &str) -> CliResult<Format> {
    let format = formats::find(to).ok_or_else(|| CliError::UnknownApp {
        name: to.to_string(),
        known: formats::known_names().join(", "),
    })?;

    if !format.writable {
        let writable: Vec<&str> = formats::writable().iter().map(|f| f.id.0).collect();
        return Err(CliError::NotWritable {
            label: format.label.to_string(),
            writable: writable.join(", "),
        });
    }

    Ok(format)
}

/// The input's name carrying the target app's extension, beside the input.
fn default_output(input: &Path, target: &Format) -> PathBuf {
    let stem = formats::stem(input);
    let name = target
        .primary_extension()
        .map_or_else(|| stem.clone(), |ext| format!("{stem}.{ext}"));
    input.with_file_name(name)
}

/// `a == b` misses `./backup.aib` against `backup.aib`, and canonicalising only
/// works on a path that exists - so try that first and fall back to the paths
/// as typed.
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn read_file(path: &Path) -> CliResult<Vec<u8>> {
    fs::read(path).map_err(|source| CliError::ReadFile {
        path: path.display().to_string(),
        source,
    })
}

/// The directory the output lands in, or `None` when there is nothing to check.
///
/// `Path::parent` answers `Some("")` for a bare filename like `a.aib`, and an
/// empty path exists nowhere - taking it at face value rejects every relative
/// output. What it means is the working directory, which is already there.
fn output_parent(output: &Path) -> Option<&Path> {
    output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
}

/// Sorted by name so two runs over the same backup print the same thing.
///
/// Counted per position in the extension table rather than per label, because
/// two extensions can share a label and each entry belongs to whichever one it
/// actually points at.
fn entries_per_extension(backup: &Backup) -> BTreeMap<String, usize> {
    let labels = report::labels(backup.extensions.iter().map(AsRef::as_ref));
    let index: HashMap<&Arc<Extension>, usize> = backup
        .extensions
        .iter()
        .enumerate()
        .map(|(at, ext)| (ext, at))
        .collect();

    let mut counts: Vec<usize> = vec![0; backup.extensions.len()];
    for entry in &backup.entries {
        // An entry whose extension is not in the table gets no row; `prune`
        // reports that as a reader bug, and `inspect` never prunes.
        if let Some(count) = index
            .get(&entry.extension)
            .and_then(|at| counts.get_mut(*at))
        {
            *count += 1;
        }
    }

    labels.into_iter().zip(counts).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use converter_core::extensions::lang::Language;
    use converter_core::extensions::types::{Extension, ExtensionRepositoryInfo};
    use converter_core::ir::types::{Manga, MangaStatus};

    use super::*;

    fn aidoku() -> Format {
        formats::find("aidoku").unwrap()
    }

    fn extension(name: &str) -> Arc<Extension> {
        Arc::new(Extension {
            reader: Tachiyomi::ID,
            name: name.into(),
            external_id: name.into(),
            url: format!("https://{name}.test"),
            lang: Language::from_code("en").unwrap(),
            repo: ExtensionRepositoryInfo::new(Tachiyomi::ID, None, None, None, None),
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

    #[test]
    fn output_keeps_the_name_and_takes_the_target_s_extension() {
        assert_eq!(
            default_output(Path::new("/tmp/library.tachibk"), &aidoku()),
            PathBuf::from("/tmp/library.aib")
        );
        // `.proto.gz` is two extensions and only one of them is Tachiyomi's.
        assert_eq!(
            default_output(Path::new("backup.proto.gz"), &aidoku()),
            PathBuf::from("backup.aib")
        );
    }

    #[test]
    fn the_source_app_comes_from_the_flag_before_the_file_name() {
        let path = Path::new("library.tachibk");

        assert_eq!(source_format(None, path).unwrap().id, Tachiyomi::ID);
        // An `.aib` misnamed `.tachibk` is still readable if the user says so.
        assert_eq!(source_format(Some("aidoku"), path).unwrap().id, Aidoku::ID);

        assert!(matches!(
            source_format(Some("kotatsu"), path),
            Err(CliError::UnknownApp { .. })
        ));
        assert!(matches!(
            source_format(None, Path::new("library.bak")),
            Err(CliError::UndetectedApp { .. })
        ));
    }

    /// Tachiyomi is import-only, so `--to tachiyomi` has to fail before the
    /// backup is read rather than at the write that would have no writer.
    #[test]
    fn an_import_only_app_is_not_a_target() {
        assert!(matches!(
            target_format("mihon"),
            Err(CliError::NotWritable { .. })
        ));
        assert_eq!(target_format("aidoku").unwrap().id, Aidoku::ID);
    }

    #[test]
    fn an_extension_with_no_entries_still_gets_a_row() {
        let used = extension("Used");
        let backup = Backup {
            reader: Tachiyomi::ID,
            version: None,
            extensions: vec![Arc::clone(&used), extension("Empty")],
            entries: vec![manga("A", Arc::clone(&used)), manga("B", used)],
            categories: Vec::new(),
        };

        let counts = entries_per_extension(&backup);
        assert_eq!(counts.get("Used (en)"), Some(&2));
        assert_eq!(counts.get("Empty (en)"), Some(&0));
    }

    /// `-o a.aib` is the ordinary way to name an output, and its parent is the
    /// empty path rather than `.`, which exists nowhere.
    #[test]
    fn a_bare_filename_has_no_parent_to_check() {
        assert_eq!(output_parent(Path::new("a.aib")), None);
        assert_eq!(
            output_parent(Path::new("out/a.aib")),
            Some(Path::new("out"))
        );
        assert_eq!(
            output_parent(Path::new("/tmp/a.aib")),
            Some(Path::new("/tmp"))
        );
    }

    /// Tachiyomi's `backupSources` carries no language, so two sources of the
    /// same app share a label; their entries belong to whichever one holds
    /// them, and merging the rows would report both counts against one.
    #[test]
    fn extensions_sharing_a_label_keep_separate_rows() {
        let first = extension("Comick");
        let second = Arc::new(Extension {
            external_id: "second".into(),
            ..first.as_ref().clone()
        });
        let backup = Backup {
            reader: Tachiyomi::ID,
            version: None,
            extensions: vec![Arc::clone(&first), Arc::clone(&second)],
            entries: vec![manga("A", Arc::clone(&first)), manga("B", second)],
            categories: Vec::new(),
        };

        let counts = entries_per_extension(&backup);
        assert_eq!(counts.len(), 2, "the two Comicks merged into one row");
        assert_eq!(counts.get("Comick (en) [Comick]"), Some(&1));
        assert_eq!(counts.get("Comick (en) [second]"), Some(&1));
    }

    #[test]
    fn a_path_is_the_same_file_however_it_is_spelled() {
        assert!(same_file(Path::new("backup.aib"), Path::new("backup.aib")));
        assert!(!same_file(Path::new("backup.aib"), Path::new("other.aib")));
    }
}
