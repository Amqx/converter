//! Rendering the pipeline's reports. Everything a conversion gave up on is
//! reported rather than errored, so the terminal is where the user finds out.

use std::collections::HashMap;
use std::sync::Arc;

use converter_core::ReaderID;
use converter_core::extensions::lang::Language;
use converter_core::extensions::matcher::RefreshReport;
use converter_core::extensions::types::Extension;
use converter_core::io::reader::ReadReport;
use converter_core::io::writer::WriteReport;
use converter_core::orchestrator::PruneReport;
use converter_core::orchestrator::mapping::{ExtensionMapping, UnresolvedReason};

/// Name and language, which is how both apps label a source in their own
/// extension lists. The language as a code rather than as a name, for the same
/// reason.
pub fn extension(extension: &Extension) -> String {
    if extension.lang == Language::Unknown {
        // `to_code` answers "all" here, and the backup never said that.
        return format!("{} (no language)", extension.name);
    }
    format!("{} ({})", extension.name, extension.lang.to_code())
}

/// The same label with the source id appended, for extensions a name alone
/// cannot tell apart.
fn extension_with_id(extension: &Extension) -> String {
    format!("{} [{}]", self::extension(extension), extension.external_id)
}

/// Labels for one list, spelling out the id only where the plain label would
/// repeat. A backup carrying two Comicks, or two sources whose names are both
/// empty because it had no `backupSources`, needs them told apart; the twenty
/// rows around them do not, and a source id on every line buries the ones that
/// matter.
pub fn labels<'a>(extensions: impl IntoIterator<Item = &'a Extension>) -> Vec<String> {
    let extensions: Vec<&Extension> = extensions.into_iter().collect();

    let mut seen: HashMap<String, usize> = HashMap::new();
    for ext in &extensions {
        *seen.entry(extension(ext)).or_default() += 1;
    }

    extensions
        .iter()
        .map(|ext| {
            let plain = extension(ext);
            if seen.get(&plain).is_some_and(|count| *count > 1) {
                extension_with_id(ext)
            } else {
                plain
            }
        })
        .collect()
}

pub fn refresh(report: &RefreshReport, for_reader: ReaderID) {
    for failure in &report.failed {
        let name = failure
            .repo
            .name
            .clone()
            .unwrap_or_else(|| failure.repo.reader.to_string());
        println!("warning: could not refresh {name}: {}", failure.error);
    }

    let relevant_failures = report
        .failed
        .iter()
        .filter(|f| f.repo.reader == for_reader)
        .count();
    let relevant_total = report
        .refreshed
        .iter()
        .filter(|r| r.reader == for_reader)
        .count()
        + relevant_failures;

    if relevant_failures > 0 {
        println!(
            "warning: {relevant_failures} of {relevant_total} repositories for the target \
             app are out of date, so extensions may look missing that are only unindexed"
        );
    }
}

pub fn read(report: &ReadReport) {
    for failed in &report.failed {
        println!(
            "warning: dropped extension {}: {}",
            extension(&failed.extension),
            failed.reason
        );
    }
    for skipped in &report.skipped {
        println!(
            "note: skipped extension {}: {}",
            extension(&skipped.extension),
            skipped.reason
        );
    }
}

pub fn mapping(mapping: &ExtensionMapping, verbose: bool) {
    for failure in mapping.search_failures() {
        let name = failure
            .repo
            .name
            .clone()
            .unwrap_or_else(|| failure.repo.reader.to_string());
        // Otherwise the extensions it would have matched are indistinguishable
        // from ones the target app genuinely does not have.
        println!(
            "warning: could not search {name}: {}; anything it indexes is \
             missing from the mapping below",
            failure.error
        );
    }

    // Collected once: `resolved()` walks a `HashMap`, so a second pass would
    // hand back a different order and the labels would stop lining up.
    let pairs: Vec<(&Arc<Extension>, &Arc<Extension>)> = mapping.resolved().collect();
    let sources = labels(
        pairs
            .iter()
            .map(|(source, _)| source.as_ref())
            .chain(mapping.unresolved().iter().map(|u| u.extension.as_ref())),
    );
    let (mapped, unmapped) = sources.split_at(pairs.len());
    let targets = labels(pairs.iter().map(|(_, target)| target.as_ref()));

    let mut resolved: Vec<(String, String)> = mapped.iter().cloned().zip(targets).collect();
    resolved.sort();

    println!(
        "extensions: {} mapped, {} unmapped",
        resolved.len(),
        mapping.unresolved().len()
    );

    if verbose {
        let width = resolved
            .iter()
            .map(|(from, _)| from.len())
            .max()
            .unwrap_or(0);
        for (from, to) in &resolved {
            println!("  {from:<width$} -> {to}");
        }
    }

    let mut unresolved: Vec<(String, String)> = mapping
        .unresolved()
        .iter()
        .zip(unmapped)
        .map(|(u, label)| {
            let reason = match u.reason {
                UnresolvedReason::NoCandidates => "nothing in the target app matched".to_string(),
                UnresolvedReason::Declined { candidates: 1 } => {
                    "1 candidate, not accepted".to_string()
                }
                UnresolvedReason::Declined { candidates } => {
                    format!("{candidates} candidates, none accepted")
                }
            };
            (label.clone(), reason)
        })
        .collect();
    unresolved.sort();

    let width = unresolved
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0);
    for (name, reason) in &unresolved {
        println!("  unmapped: {name:<width$}  {reason}");
    }
}

pub fn prune(report: &PruneReport, kept: usize) {
    println!("entries: {kept} kept, {} dropped", report.dropped());
}

pub fn write(report: &WriteReport) {
    for failed in &report.failed {
        println!(
            "warning: could not write extension {}: {}",
            extension(&failed.extension),
            failed.reason
        );
    }
    for skipped in &report.skipped {
        println!(
            "note: skipped extension {}: {}",
            extension(&skipped.extension),
            skipped.reason
        );
    }
    for dropped in &report.dropped {
        println!(
            "warning: dropped {} rows from {}: {} (e.g. {})",
            dropped.count,
            extension(&dropped.extension),
            dropped.reason,
            dropped.examples.join(", ")
        );
    }
}

/// Rounded to one decimal, in integer arithmetic - a cast to `f64` to print a
/// file size is exactly the kind the workspace bans.
pub fn size(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = KIB * 1024;

    match bytes {
        b if b >= MIB => format!("{}.{} MiB", b / MIB, (b % MIB) * 10 / MIB),
        b if b >= KIB => format!("{}.{} KiB", b / KIB, (b % KIB) * 10 / KIB),
        b => format!("{b} bytes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_the_way_a_file_manager_writes_them() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(999), "999 bytes");
        assert_eq!(size(1024), "1.0 KiB");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(1024 * 1024), "1.0 MiB");
        assert_eq!(size(3 * 1024 * 1024 + 512 * 1024), "3.5 MiB");
    }
}
