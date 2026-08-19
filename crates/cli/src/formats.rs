//! Which apps this binary knows about, and how to recognise their files.

use std::path::Path;

use converter_aidoku::Aidoku;
use converter_core::ReaderID;
use converter_core::format::BackupFormat;
use converter_tachi::Tachiyomi;

/// One app, plus what a command line needs that a
/// [`BackupFormat`] doesn't carry: what to call it and what its files
/// are named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    pub id: ReaderID,
    /// What to print. The id is the spelling to type.
    pub label: &'static str,
    /// Other spellings `--from`/`--to` accept, for apps known by several names.
    pub aliases: &'static [&'static str],
    /// Lowercase, no leading dot. The first is what `convert` names its output.
    pub file_extensions: &'static [&'static str],
    /// Read off the format itself rather than written down here, so an
    /// import-only app can never be offered as a target.
    pub writable: bool,
}

impl Format {
    pub fn matches(&self, name: &str) -> bool {
        let name = name.trim();
        self.id.0.eq_ignore_ascii_case(name)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    }

    /// The extension `convert` gives a file it writes for this app.
    pub const fn primary_extension(&self) -> Option<&'static str> {
        self.file_extensions.first().copied()
    }
}

fn describe<F: BackupFormat>(
    label: &'static str,
    aliases: &'static [&'static str],
    file_extensions: &'static [&'static str],
) -> Format {
    Format {
        id: F::id(),
        label,
        aliases,
        file_extensions,
        writable: F::writer().is_some(),
    }
}

/// Every format the binary is built with, in the order commands list them.
pub fn all() -> Vec<Format> {
    vec![
        describe::<Tachiyomi>(
            "Tachiyomi / Mihon / SY",
            // The fork a backup came from is only implied by the fields in it,
            // so all three names resolve to the one reader.
            &["mihon", "tachi", "sy", "tachiyomisy"],
            &["tachibk", "proto.gz"],
        ),
        describe::<Aidoku>("Aidoku", &[], &["aib"]),
    ]
}

pub fn find(name: &str) -> Option<Format> {
    all().into_iter().find(|format| format.matches(name))
}

/// The apps `--to` will take.
pub fn writable() -> Vec<Format> {
    all().into_iter().filter(|f| f.writable).collect()
}

/// Every spelling the format arguments accept, for an error message.
pub fn known_names() -> Vec<&'static str> {
    all().iter().map(|f| f.id.0).collect()
}

/// Guesses the app from the file name alone. Both apps use an extension of
/// their own, so this is right whenever the file still has the name the app
/// gave it - and `read` still has to agree before anything is converted.
pub fn detect(path: &Path) -> Option<Format> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    all().into_iter().find(|format| {
        format
            .file_extensions
            .iter()
            .any(|ext| ends_with_ext(&name, ext))
    })
}

/// The part of `name` before whichever app extension it carries. Plain
/// [`Path::file_stem`] would leave the `.proto` on a `backup.proto.gz`.
pub fn stem(path: &Path) -> String {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return String::new();
    };
    let lower = name.to_lowercase();

    for format in all() {
        for ext in format.file_extensions {
            if ends_with_ext(&lower, ext) {
                let keep = name.len().saturating_sub(ext.len() + 1);
                return name.get(..keep).unwrap_or(&name).to_string();
            }
        }
    }

    path.file_stem()
        .map_or(name, |s| s.to_string_lossy().into_owned())
}

/// The name has to have something in front of the extension: a bare `.aib` is
/// a dotfile, not a backup called nothing.
fn ends_with_ext(lowercase_name: &str, ext: &str) -> bool {
    lowercase_name.len() > ext.len() + 1 && lowercase_name.ends_with(&format!(".{ext}"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn writability_comes_from_the_format_not_this_table() {
        let tachi = find("tachiyomi").unwrap();
        let aidoku = find("aidoku").unwrap();

        // Tachiyomi is import-only: `declare_format!` gives it no writer.
        assert!(!tachi.writable);
        assert!(aidoku.writable);
        assert_eq!(writable(), vec![aidoku]);
    }

    #[test]
    fn names_and_aliases_are_case_insensitive() {
        assert_eq!(find("Mihon").unwrap().id, Tachiyomi::ID);
        assert_eq!(find("TACHIYOMI").unwrap().id, Tachiyomi::ID);
        assert_eq!(find(" aidoku ").unwrap().id, Aidoku::ID);
        assert!(find("kotatsu").is_none());
    }

    #[test]
    fn detection_reads_the_whole_extension() {
        let tachi = |p: &str| detect(&PathBuf::from(p)).map(|f| f.id);

        assert_eq!(tachi("library.tachibk"), Some(Tachiyomi::ID));
        assert_eq!(tachi("/tmp/LIBRARY.TachiBK"), Some(Tachiyomi::ID));
        // A `.gz` alone is not enough to claim it is Tachiyomi's.
        assert_eq!(tachi("backup.proto.gz"), Some(Tachiyomi::ID));
        assert_eq!(tachi("backup.gz"), None);
        assert_eq!(tachi("backup.aib"), Some(Aidoku::ID));
        assert_eq!(tachi("backup"), None);
        // A name that is nothing but the extension is a dotfile, not a backup.
        assert_eq!(tachi(".aib"), None);
    }

    #[test]
    fn stem_drops_the_whole_extension() {
        assert_eq!(stem(&PathBuf::from("library.tachibk")), "library");
        assert_eq!(stem(&PathBuf::from("/tmp/backup.proto.gz")), "backup");
        assert_eq!(stem(&PathBuf::from("backup.aib")), "backup");
        // Unknown extensions fall back to the usual stem.
        assert_eq!(stem(&PathBuf::from("backup.tar.gz")), "backup.tar");
        assert_eq!(stem(&PathBuf::from("backup")), "backup");
    }
}
