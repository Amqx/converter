//! The command line itself: what the binary accepts, and the policy the flags
//! add up to.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use converter_core::extensions::types::{Extension, ExtensionMatch};
use converter_core::orchestrator::policy::{AutoBest, Resolution, ResolutionPolicy, Strict};

/// Converts a manga reader's backup into another reader's format, re-pointing
/// every entry at the equivalent extension in the target app.
#[derive(Debug, Parser)]
#[command(name = "converter", version, about, long_about = None)]
#[command(arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Convert a backup to another app's format.
    Convert(ConvertArgs),
    /// Read a backup and describe what is in it, without converting.
    Inspect(InspectArgs),
    /// List the apps this build knows how to read and write.
    Formats,
}

#[derive(Debug, clap::Args)]
pub struct ConvertArgs {
    /// The backup to read.
    #[arg(value_name = "BACKUP", value_hint = clap::ValueHint::FilePath)]
    pub input: PathBuf,

    /// The app to convert to.
    #[arg(short, long, value_name = "APP")]
    pub to: String,

    /// The app the backup came from. Guessed from the file name otherwise.
    #[arg(short, long, value_name = "APP")]
    pub from: Option<String>,

    /// Where to write the converted backup. Defaults to the input's name with
    /// the target app's extension.
    #[arg(short, long, value_name = "PATH", value_hint = clap::ValueHint::FilePath)]
    pub output: Option<PathBuf>,

    /// How to pick a target extension when several could match.
    #[arg(long, value_enum, default_value_t = PolicyKind::Auto)]
    pub policy: PolicyKind,

    /// Lowest score `auto` will accept, from 0 to 1.
    #[arg(long, value_name = "SCORE", value_parser = score, default_value_t = AutoBest::default().min_score)]
    pub min_score: f32,

    /// Do everything but write the file.
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Overwrite the output file if it already exists.
    // No short form: `-f` is `--from`, which a conversion reaches for far more
    // often than it overwrites anything.
    #[arg(long)]
    pub force: bool,

    /// List every extension as it is mapped, not just the ones that failed.
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Debug, clap::Args)]
pub struct InspectArgs {
    /// The backup to read.
    #[arg(value_name = "BACKUP", value_hint = clap::ValueHint::FilePath)]
    pub input: PathBuf,

    /// The app the backup came from. Guessed from the file name otherwise.
    #[arg(short, long, value_name = "APP")]
    pub from: Option<String>,

    /// List every extension and entry count, not just the totals.
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PolicyKind {
    /// Take the best candidate that clears `--min-score`, skip the source
    /// otherwise.
    Auto,
    /// Take a lone exact match, skip when there is none, and stop the whole
    /// conversion rather than guess between two.
    Strict,
}

impl PolicyKind {
    pub const fn with(self, min_score: f32) -> Policy {
        match self {
            Self::Auto => Policy::Auto(AutoBest { min_score }),
            Self::Strict => Policy::Strict(Strict),
        }
    }
}

/// The chosen policy as one type, because `orchestrator!` is built around a
/// single concrete one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Policy {
    Auto(AutoBest),
    Strict(Strict),
}

impl ResolutionPolicy for Policy {
    fn resolve(&self, source: &Extension, candidates: &[ExtensionMatch]) -> Resolution {
        match self {
            Self::Auto(auto) => auto.resolve(source, candidates),
            Self::Strict(strict) => strict.resolve(source, candidates),
        }
    }
}

/// Scores are `0.0..=1.0`; anything outside silently accepts everything or
/// nothing, which is never what was meant.
fn score(raw: &str) -> Result<f32, String> {
    let score: f32 = raw
        .parse()
        .map_err(|_| format!("`{raw}` is not a number"))?;
    if (0.0..=1.0).contains(&score) {
        Ok(score)
    } else {
        Err(format!("`{raw}` is not between 0 and 1"))
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    use super::*;

    #[test]
    fn the_command_tree_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn min_score_defaults_to_the_policy_s_own_bar() {
        let cli = Cli::parse_from(["converter", "convert", "backup.tachibk", "--to", "aidoku"]);
        let Command::Convert(args) = cli.command else {
            panic!("parsed the wrong subcommand");
        };

        assert_eq!(args.min_score, AutoBest::default().min_score);
        assert_eq!(args.policy, PolicyKind::Auto);
        assert_eq!(args.from, None);
    }

    #[test]
    fn scores_outside_the_range_are_rejected() {
        assert!(score("0").is_ok());
        assert!(score("0.8").is_ok());
        assert!(score("1").is_ok());
        assert!(score("1.5").is_err());
        assert!(score("-0.1").is_err());
        assert!(score("high").is_err());
    }

    #[test]
    fn the_flags_pick_the_policy() {
        assert_eq!(
            PolicyKind::Auto.with(0.5),
            Policy::Auto(AutoBest { min_score: 0.5 })
        );
        assert_eq!(PolicyKind::Strict.with(0.5), Policy::Strict(Strict));
    }
}
