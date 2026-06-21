mod converter;
mod matching;
mod reader;
mod report;

use crate::converter::aidoku::AidokuConverter;
use crate::converter::tachiyomi::TachiyomiConverter;
use crate::converter::{BackupReader, BackupStats, BackupWriter};
use crate::matching::Matcher;
use crate::matching::aidoku::AidokuSourceList;
use crate::matching::tachiyomi::TachiyomiExtensionRepo;
use crate::reader::ReaderId;
use crate::reader::ReaderId::{Aidoku, Tachiyomi};
use crate::report::{ConversionReport};
use clap::{ArgAction, ArgGroup, Parser};
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use tracing::level_filters::LevelFilter;
use tracing::{debug, error, info, warn};
use tracing_subscriber::FmtSubscriber;
use chrono::{Local};

#[derive(Parser, Debug)]
#[command(group(
    ArgGroup::new("input_source")
        .args(["input", "input_flag"])
        .required(true)
))]
#[command(group(
    ArgGroup::new("output_target")
        .args(["output", "to"])
        .required(true)
))]
#[command(
    name = "Converter",
    about = "Tool for converting backups from one reader to another"
)]
struct Args {
    /// Input file
    #[arg(short = 'i', long = "input", conflicts_with = "input", value_parser = input_check)]
    input_flag: Option<FileArg>,

    /// Positional input
    #[arg(conflicts_with = "input_flag", value_parser = input_check)]
    input: Option<FileArg>,

    /// Output file
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    output: Option<PathBuf>,

    /// Output target type. Overrides output file extension when both are present
    #[arg(short = 't', long = "to", value_enum)]
    to: Option<ReaderId>,

    /// Add custom Aidoku repositories
    #[arg(long = "aidoku-repo", value_name = "REPO", num_args = 1..)]
    aidoku_repo: Vec<String>,

    /// Add custom Tachiyomi repositories
    #[arg(long = "tachi-repo", value_name = "REPO", num_args = 1..)]
    tachi_repo: Vec<String>,

    /// Increase verbosity
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count)]
    verbose: u8,

    /// Do not include default repositories/config
    #[arg(long = "no-defaults", action = ArgAction::SetTrue)]
    no_defaults: bool,
}

#[derive(Clone, Debug)]
struct FileArg {
    pub path: Option<PathBuf>,
    pub kind: ReaderId,
}

fn input_check(s: &str) -> Result<FileArg, String> {
    let path = PathBuf::from(s);
    match path.extension().and_then(|e| e.to_str()) {
        Some("json" | "aib") => Ok(FileArg {
            path: Some(path),
            kind: Aidoku,
        }),
        Some("tachibk") => Ok(FileArg {
            path: Some(path),
            kind: Tachiyomi,
        }),
        _ => Err(format!(
            "'{}' isn't a valid input format! Looking for:\n\
              - '.aib/ .json' for Aidoku\
              - '.tachibk' for Tachiyomi ",
            path.display()
        )),
    }
}

fn input_parse(args: &Args) -> FileArg {
    args.input
        .clone()
        .or_else(|| args.input_flag.clone())
        .expect("Clap ArgGroup ensures either input or input_flag is present")
}

fn output_parse(args: &Args) -> FileArg {
    let target_type = if let Some(explicit_target) = args.to {
        explicit_target
    } else if let Some(ref output_path) = args.output {
        match output_path.extension().and_then(|e| e.to_str()) {
            Some("json" | "aib") => Aidoku,
            Some("tachibk") => Tachiyomi,
            _ => {
                error!(
                    "Error: Could not infer target format from output file extension. Define with -o or -t."
                );
                std::process::exit(1);
            }
        }
    } else {
        unreachable!("Clap ArgGroup ensures either -o or -t is present");
    };

    let output_path = args.output.clone().unwrap_or_else(|| match target_type {
        Aidoku => PathBuf::from("output.aib"),
        Tachiyomi => PathBuf::from("output.tachibk"),
    });

    FileArg {
        path: Some(output_path),
        kind: target_type,
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let input = input_parse(&args);
    let output = output_parse(&args);

    if input.kind == output.kind {
        return Err("Input and output cannot be the same format.".into());
    }

    let verbosity = args.verbose;
    let subscriber = FmtSubscriber::builder()
        .with_max_level(match verbosity {
            0 => LevelFilter::WARN,
            1 => LevelFilter::INFO,
            2 => LevelFilter::DEBUG,
            _ => LevelFilter::TRACE,
        })
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let input_path = input.path.expect("Should be a path at this point");
    let output_path = output.path.expect("Should be a path at this point");

    info!(
        input_format = ?input.kind,
        output_format = ?output.kind,
        "Starting backup conversion"
    );
    debug!(
        input_path = %input_path.display(),
        output_path = %output_path.display(),
        "Resolved conversion paths"
    );

    // make sure there's at least 1 repo to match between
    let mut matcher = Matcher::new();
    let no_defaults = args.no_defaults;
    let aidoku_repos = args.aidoku_repo;
    let tachi_repos = args.tachi_repo;
    if no_defaults {
        info!("Default source repositories disabled");
        if aidoku_repos.is_empty() || tachi_repos.is_empty() {
            return Err("Not enough repos for matching, either use defaults or provide your own with -aidoku-repo and -tachi-repo".into());
        }
        debug!(
            aidoku_repos = aidoku_repos.len(),
            tachiyomi_repos = tachi_repos.len(),
            "Validated user-provided source repository arguments"
        );
    } else {
        info!("Loading default source repositories for matching");
        let tachi_default = TachiyomiExtensionRepo::default()?;
        debug!("Loaded default Tachiyomi source repository");
        let aidoku_default = AidokuSourceList::default()?;
        debug!("Loaded default Aidoku source list");

        matcher.register_source(Box::new(tachi_default));
        matcher.register_source(Box::new(aidoku_default));
    }
    for aidoku_repo in aidoku_repos {
        matcher.register_source(Box::new(AidokuSourceList::new(aidoku_repo)?));
    }
    for tachi_repo in tachi_repos {
        matcher.register_source(Box::new(TachiyomiExtensionRepo::new(tachi_repo)?));
    }

    info!("Reading input backup");
    let input_file = fs::read(&input_path)?;
    debug!(bytes = input_file.len(), "Read input backup bytes");

    info!(input_format = ?input.kind, "Parsing input backup");
    let (input_backup, read_report) = match input.kind {
        Tachiyomi => TachiyomiConverter::read_backup(&input_file)?,
        Aidoku => AidokuConverter::read_backup(&input_file)?,
    };
    let input_stats = BackupStats::from_backup(&input_backup);
    info!(
        manga = input_stats.manga,
        categories = input_stats.categories,
        "Parsed input backup"
    );
    debug!(
        chapters = input_stats.chapters,
        history_entries = input_stats.history_entries,
        "Parsed input backup details"
    );

    info!(output_format = ?output.kind, "Serializing output backup");
    let (output_bytes, write_report) = match output.kind {
        Tachiyomi => TachiyomiConverter::write_backup(&input_backup, &matcher)?,
        Aidoku => AidokuConverter::write_backup(&input_backup, &matcher)?,
    };
    debug!(bytes = output_bytes.len(), "Serialized output backup bytes");

    fs::write(&output_path, output_bytes)?;
    info!("Wrote output backup to `{}`", output_path.display());
    info!("Finished backup conversion");

    let report_path = {
        let mut p = output_path.clone().into_os_string();
        p.push(".report.txt");
        PathBuf::from(p)
    };
    let report = ConversionReport {
        timestamp: Local::now().format("%Y-%m-%d %H:%M %Z").to_string(),
        input_file: input_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| input_path.display().to_string()),
        output_file: output_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| output_path.display().to_string()),
        from: input.kind,
        to: output.kind,
        repos: matcher.repos(),
        read: read_report,
        write: write_report,
    };
    match fs::write(&report_path, report.render()) {
        Ok(()) => info!("Wrote conversion report to `{}`", report_path.display()),
        Err(err) => warn!(
            error = %err,
            path = %report_path.display(),
            "Failed to write conversion report (conversion itself succeeded)"
        ),
    }

    Ok(())
}
