//! Boilerplate every backend crate would otherwise copy: naming a format and
//! hooking its reader, writer and repositories up to an orchestrator.

/// Declares a backup format: a marker type, its [`ReaderID`](crate::ReaderID),
/// and a [`BackupFormat`](crate::format::BackupFormat) impl.
///
/// `writer` and `repositories` are both optional: leave the writer out for an
/// import-only format, and the repositories out for one nothing can target.
///
/// ```ignore
/// converter_core::declare_format! {
///     pub Tachiyomi = "tachiyomi" {
///         reader: TachiReader::new(),
///         writer: TachiWriter::new(),
///         repositories: [TachiRepo::official(), TachiRepo::keiyoushi()],
///     }
/// }
/// ```
///
/// The id also lands on the type as a const, so backends can say
/// `Tachiyomi::ID` instead of repeating the string everywhere.
#[macro_export]
macro_rules! declare_format {
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident = $id:literal {
            reader: $reader:expr
            $(, writer: $writer:expr)?
            $(, repositories: [$($repo:expr),* $(,)?])?
            $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
        $vis struct $name;

        impl $name {
            pub const ID: $crate::ReaderID = $crate::ReaderID($id);
        }

        impl $crate::format::BackupFormat for $name {
            fn id() -> $crate::ReaderID {
                Self::ID
            }

            fn reader() -> ::std::boxed::Box<dyn $crate::io::reader::BackupReader> {
                ::std::boxed::Box::new($reader)
            }

            fn writer(
            ) -> ::std::option::Option<::std::boxed::Box<dyn $crate::io::writer::BackupWriter>> {
                $crate::__format_writer!($($writer)?)
            }

            fn repositories(
            ) -> ::std::vec::Vec<
                ::std::boxed::Box<dyn $crate::extensions::types::ExtensionRepository>,
            > {
                ::std::vec![$($(
                    ::std::boxed::Box::new($repo)
                ),*)?]
            }
        }
    };
}

/// The `reader()` method both [`BackupReader`] and [`BackupWriter`] need,
/// pulling the id off a format declared with [`declare_format!`] instead of
/// spelling it out again.
///
/// ```ignore
/// impl BackupReader for TachiReader {
///     converter_core::format_id!(Tachiyomi);
///
///     fn read_backup(&self, bytes: &[u8]) -> Result<(Backup, ReadReport), ReadError> {
///         // ...
///     }
/// }
/// ```
///
/// [`BackupReader`]: crate::io::reader::BackupReader
/// [`BackupWriter`]: crate::io::writer::BackupWriter
#[macro_export]
macro_rules! format_id {
    ($format:ty) => {
        fn reader(&self) -> $crate::ReaderID {
            <$format as $crate::format::BackupFormat>::id()
        }
    };
}

/// Builds an [`Orchestrator`](crate::orchestrator::Orchestrator) from a policy
/// and a list of formats, registering each format's reader, writer and repos.
///
/// If any repo fetches its index over the network, `refresh()` the result
/// before converting anything.
///
/// ```ignore
/// let mut orchestrator = converter_core::orchestrator! {
///     policy: AutoBest::default(),
///     formats: [Tachiyomi, Aidoku],
/// };
/// let refresh = orchestrator.refresh().await;
/// if !refresh.is_complete() {
///     // Some extensions will look missing that are merely unindexed.
/// }
/// ```
#[macro_export]
macro_rules! orchestrator {
    (
        policy: $policy:expr,
        formats: [$($format:ty),* $(,)?] $(,)?
    ) => {{
        let mut orchestrator = $crate::orchestrator::Orchestrator::new(
            $crate::extensions::matcher::ExtensionMatcher::default(),
            ::std::boxed::Box::new($policy),
        );
        $(orchestrator.register_format::<$format>();)*
        orchestrator
    }};
}

/// `declare_format!`'s optional `writer` field, as an `Option`.
#[doc(hidden)]
#[macro_export]
macro_rules! __format_writer {
    () => {
        ::std::option::Option::None
    };
    ($writer:expr) => {
        ::std::option::Option::Some(::std::boxed::Box::new($writer))
    };
}

#[cfg(test)]
mod tests {
    use crate::ReaderID;
    use crate::extensions::ExtResult;
    use crate::extensions::types::{
        ExtensionMatch, ExtensionQuery, ExtensionRepository, ExtensionRepositoryInfo,
    };
    use crate::format::BackupFormat;
    use crate::io::reader::{BackupReader, ReadReport};
    use crate::io::writer::{BackupWriter, WriteReport};
    use crate::io::{ReadError, WriteError};
    use crate::ir::types::Backup;
    use crate::orchestrator::OrchestratorError;
    use crate::orchestrator::mapping::ExtensionMapping;
    use crate::orchestrator::policy::AutoBest;

    struct StubReader;
    struct StubImportReader;
    struct StubWriter;
    struct StubRepo(ExtensionRepositoryInfo);

    impl BackupReader for StubReader {
        crate::format_id!(Exportable);

        fn read_backup(&self, _bytes: &[u8]) -> Result<(Backup, ReadReport), ReadError> {
            Ok((
                Backup {
                    reader: Exportable::ID,
                    version: None,
                    extensions: Vec::new(),
                    entries: Vec::new(),
                    categories: Vec::new(),
                },
                ReadReport::default(),
            ))
        }
    }

    impl BackupReader for StubImportReader {
        crate::format_id!(ImportOnly);

        fn read_backup(&self, _bytes: &[u8]) -> Result<(Backup, ReadReport), ReadError> {
            Ok((
                Backup {
                    reader: ImportOnly::ID,
                    version: None,
                    extensions: Vec::new(),
                    entries: Vec::new(),
                    categories: Vec::new(),
                },
                ReadReport::default(),
            ))
        }
    }

    impl BackupWriter for StubWriter {
        crate::format_id!(Exportable);

        fn write_backup(
            &self,
            _backup: &Backup,
            _mapping: &ExtensionMapping,
        ) -> Result<(Vec<u8>, WriteReport), WriteError> {
            Ok((Vec::new(), WriteReport::default()))
        }
    }

    impl ExtensionRepository for StubRepo {
        fn info(&self) -> &ExtensionRepositoryInfo {
            &self.0
        }

        fn search(&self, _query: &ExtensionQuery) -> ExtResult<Vec<ExtensionMatch>> {
            Ok(Vec::new())
        }
    }

    fn repo() -> StubRepo {
        StubRepo(ExtensionRepositoryInfo {
            reader: Exportable::ID,
            name: None,
            short_name: None,
            website: None,
            signing_key: None,
        })
    }

    crate::declare_format! {
        /// Round-trips.
        pub Exportable = "exportable" {
            reader: StubReader,
            writer: StubWriter,
            repositories: [repo()],
        }
    }

    crate::declare_format! {
        /// Read-only: no writer, no repos.
        pub ImportOnly = "import-only" {
            reader: StubImportReader,
        }
    }

    #[test]
    fn id_lands_on_both_the_type_and_the_trait() {
        assert_eq!(Exportable::ID, ReaderID("exportable"));
        assert_eq!(Exportable::id(), Exportable::ID);
        assert_eq!(ImportOnly::ID, ReaderID("import-only"));
    }

    #[test]
    fn omitted_fields_come_back_empty() {
        assert!(Exportable::writer().is_some());
        assert_eq!(Exportable::repositories().len(), 1);

        assert!(ImportOnly::writer().is_none());
        assert!(ImportOnly::repositories().is_empty());
    }

    #[test]
    fn format_id_reports_the_declared_id() {
        assert_eq!(StubReader.reader(), Exportable::ID);
        assert_eq!(StubWriter.reader(), Exportable::ID);
    }

    #[test]
    fn orchestrator_macro_wires_everything_up() {
        let orchestrator = crate::orchestrator! {
            policy: AutoBest::default(),
            formats: [Exportable],
        };

        assert_eq!(orchestrator.matcher().registered().len(), 1);
        assert!(orchestrator.read(b"", Exportable::ID).is_ok());

        let backup = orchestrator.read(b"", Exportable::ID).unwrap().0;
        let mapping = orchestrator.resolve(&backup, Exportable::ID).unwrap();
        assert!(
            orchestrator
                .write(&backup, &mapping, Exportable::ID)
                .is_ok()
        );
    }

    /// `declare_format!` takes any expression for its reader, so nothing stops
    /// a backend pointing `format_id!` at the wrong format, after which
    /// dispatch just never finds it. Registration checks that the two agree.
    #[test]
    #[should_panic(expected = "reader reports a different format")]
    fn mismatched_reader_panics_at_registration() {
        crate::declare_format! {
            pub Mismatched = "mismatched" {
                reader: StubReader,
            }
        }

        let _ = crate::orchestrator! {
            policy: AutoBest::default(),
            formats: [Mismatched],
        };
    }

    #[test]
    fn import_only_format_has_no_writer() {
        let orchestrator = crate::orchestrator! {
            policy: AutoBest::default(),
            formats: [ImportOnly],
        };
        let backup = orchestrator.read(b"", ImportOnly::ID).unwrap().0;
        let mapping = ExtensionMapping::new(ImportOnly::ID);

        assert!(matches!(
            orchestrator.write(&backup, &mapping, ImportOnly::ID),
            Err(OrchestratorError::NoWriterFor(_))
        ));
    }
}
