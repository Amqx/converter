mod converter;
mod matching;

use std::error::Error;
use std::fs;
use clap::{ArgAction, Parser, ArgGroup};
use std::path::PathBuf;
use crate::converter::aidoku::AidokuConverter;
use crate::converter::{BackupReader, BackupWriter};
use crate::converter::tachiyomi::TachiyomiConverter;
use crate::matching::{Matcher, SourceId};
use crate::matching::aidoku::AidokuSourceList;
use crate::matching::SourceId::{Aidoku, Tachiyomi};
use crate::matching::tachiyomi::TachiyomiExtensionRepo;

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
struct Args {
    /// Input file, also accepted positionally
    #[arg(short = 'i', long = "input", conflicts_with = "input", value_parser = input_check)]
    input_flag: Option<FileArg>,

    /// Positional input file, equivalent to --input
    #[arg(conflicts_with = "input_flag", value_parser = input_check)]
    input: Option<FileArg>,

    /// Output file
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    output: Option<PathBuf>,

    /// Output target type. Overrides output file extension when both are present.
    #[arg(short = 't', long = "to", value_enum)]
    to: Option<SourceId>,

    /// Add customn Aidoku repositories
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
    pub kind: SourceId
}

fn input_check(s: &str) -> Result<FileArg, String> {
    let path = PathBuf::from(s);
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") | Some("aib") => Ok(FileArg {path: Some(path), kind: Aidoku}),
        Some("tachibk") => Ok(FileArg {path: Some(path), kind: Tachiyomi}),
        _ => Err(format!(
            "'{}' isn't a valid input format! Looking for:\n\
              - '.aib/ .json' for Aidoku\
              - '.tachibk' for Tachiyomi ", path.display()
        ))
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
            Some("json") | Some("aib") => Aidoku,
            Some("tachibk") => Tachiyomi,
            _ => {
                eprintln!("Error: Could not infer target format from output file extension.");
                std::process::exit(1);
            }
        }
    } else {
        unreachable!("Clap ArgGroup ensures either -o or -t is present");
    };

    let output_path = args.output.clone().unwrap_or_else(|| {
        match target_type {
            Aidoku => PathBuf::from("output.aib"),
            Tachiyomi => PathBuf::from("output.tachibk"),
        }
    });

    FileArg { path: Some(output_path), kind: target_type }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let input = input_parse(&args);
    let output = output_parse(&args);

    let verbosity = args.verbose;

    // make sure there's at least 1 repo to match between
    let mut matcher = Matcher::new();
    let no_defaults = args.no_defaults;
    let aidoku_repos = args.aidoku_repo;
    let tachi_repos = args.tachi_repo;
    if no_defaults {
        if aidoku_repos.is_empty() || tachi_repos.is_empty() {
            return Err("Not enough repos for matching, either use defaults or provide your own with -aidoku-repo and -tachi-repo".into());
        }
    } else {
        let tachi_default = TachiyomiExtensionRepo::default()?;
        let aidoku_default = AidokuSourceList::default()?;

        matcher.register_source(Box::new(tachi_default));
        matcher.register_source(Box::new(aidoku_default));
    }

    let input_file = fs::read(input.path.expect("Should be a path at this point"))?;
    let input_backup = match input.kind {
        Tachiyomi => TachiyomiConverter::read_backup(&input_file)?,
        Aidoku => AidokuConverter::read_backup(&input_file)?,
    };
    let output_bytes = match output.kind {
        Tachiyomi => TachiyomiConverter::write_backup(&input_backup, &matcher)?,
        Aidoku => AidokuConverter::write_backup(&input_backup, &matcher)?,
    };

    let output_path = output.path.expect("Should be a path at this point");
    fs::write(&output_path, output_bytes)?;

    println!("Wrote out backup to `{}`", output_path.display());
    Ok(())
}
