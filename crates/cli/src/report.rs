//! Rendering the pipeline's reports. Everything a conversion gave up on is
//! reported rather than errored, so the terminal is where the user finds out.

use converter_core::extensions::lang::Language;
use converter_core::extensions::matcher::RefreshReport;
use converter_core::extensions::types::Extension;
use converter_core::io::reader::ReadReport;
use converter_core::io::writer::WriteReport;
use converter_core::orchestrator::PruneReport;
use converter_core::orchestrator::mapping::{ExtensionMapping, UnresolvedReason};

/// Name and language, which together are what tells two extensions apart. The
/// language as a code rather than as a name, because that is how both apps
/// label the source in their own extension lists.
pub fn extension(extension: &Extension) -> String {
    if extension.lang == Language::Unknown {
        // `to_code` answers "all" here, and the backup never said that.
        return format!("{} (no language)", extension.name);
    }
    format!("{} ({})", extension.name, extension.lang.to_code())
}

pub fn refresh(report: &RefreshReport) {
    for failure in &report.failed {
        let name = failure
            .repo
            .name
            .clone()
            .unwrap_or_else(|| failure.repo.reader.to_string());
        println!("warning: could not refresh {name}: {}", failure.error);
    }

    if !report.is_complete() {
        // Worth saying outright: an unindexed extension and a missing one look
        // exactly alike by the time resolution is done with them.
        println!(
            "warning: {} of {} repositories are out of date, so extensions may look missing \
             that are only unindexed",
            report.failed.len(),
            report.failed.len() + report.refreshed.len()
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
    let mut resolved: Vec<(String, String)> = mapping
        .resolved()
        .map(|(source, target)| (extension(source), extension(target)))
        .collect();
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
        .map(|u| {
            let reason = match u.reason {
                UnresolvedReason::NoCandidates => "nothing in the target app matched".to_string(),
                UnresolvedReason::Declined { candidates: 1 } => {
                    "1 candidate, not accepted".to_string()
                }
                UnresolvedReason::Declined { candidates } => {
                    format!("{candidates} candidates, none accepted")
                }
            };
            (extension(&u.extension), reason)
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
