use crate::extensions::{ExtResult, ExtensionError};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Language {
    /// BCP-47 style tags: a primary language, plus the script and region.
    Tagged {
        iso: isolang::Language,
        /// Script subtag in canonical Titlecase, e.g. `Hans`.
        script: Option<String>,
        /// Region subtag in canonical uppercase, e.g. `BR`, or a UN M.49 code.
        region: Option<String>,
    },
    All,
    /// The backup never said. Distinct from [`Self::All`], which is a claim the source makes
    /// about itself: some formats (Tachiyomi's `backupSources`) carry no language field at all,
    /// and scoring an absence as if it were "serves every language" penalises every candidate.
    /// `from_code` never produces this - a code that parses is always an answer.
    Unknown,
}

/// No two apps spell "this source serves every language" the same way.
const ANY_LANGUAGE_CODES: [&str; 4] = ["all", "mul", "mult", "multi"];

impl Language {
    /// Parses `en`, `jpn`, `pt-BR`, `zh_Hant_HK`, `all` and friends. Case and
    /// separator (`-` or `_`) don't matter, and the primary subtag is tried as
    /// ISO 639-1 before 639-3.
    pub fn from_code(code: &str) -> ExtResult<Self> {
        let trimmed = code.trim();
        if ANY_LANGUAGE_CODES
            .iter()
            .any(|any| trimmed.eq_ignore_ascii_case(any))
        {
            return Ok(Self::All);
        }

        let unknown = || ExtensionError::UnknownLanguageCode(code.to_string());

        let mut subtags = trimmed.split(['-', '_']).filter(|part| !part.is_empty());
        let primary = subtags.next().ok_or_else(unknown)?.to_ascii_lowercase();
        let iso = isolang::Language::from_639_1(&primary)
            .or_else(|| isolang::Language::from_639_3(&primary))
            .ok_or_else(unknown)?;

        let mut script = None;
        let mut region = None;
        for subtag in subtags {
            if script.is_none() && is_script(subtag) {
                script = Some(titlecase(subtag));
            } else if region.is_none() && is_region(subtag) {
                region = Some(subtag.to_ascii_uppercase());
            }
        }

        Ok(Self::Tagged {
            iso,
            script,
            region,
        })
    }

    /// Back to a code we can write into a backup. Prefers the two-letter form,
    /// falling back to 639-3 for languages that don't have one.
    pub fn to_code(&self) -> String {
        match self {
            // A writer has to put *something* in the field, and "all" is the one code every app
            // reads as "do not filter on this" - the closest thing to saying nothing.
            Self::All | Self::Unknown => "all".to_string(),
            Self::Tagged {
                iso,
                script,
                region,
            } => {
                let mut code = iso.to_639_1().unwrap_or_else(|| iso.to_639_3()).to_string();
                if let Some(script) = script {
                    code.push('-');
                    code.push_str(script);
                }
                if let Some(region) = region {
                    code.push('-');
                    code.push_str(region);
                }
                code
            }
        }
    }
}

fn is_script(subtag: &str) -> bool {
    subtag.len() == 4 && subtag.chars().all(|c| c.is_ascii_alphabetic())
}

fn is_region(subtag: &str) -> bool {
    (subtag.len() == 2 && subtag.chars().all(|c| c.is_ascii_alphabetic()))
        || (subtag.len() == 3 && subtag.chars().all(|c| c.is_ascii_digit()))
}

fn titlecase(subtag: &str) -> String {
    let mut chars = subtag.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
    })
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => write!(f, "Multilingual (all)"),
            Self::Unknown => write!(f, "Unknown"),
            Self::Tagged {
                iso,
                script,
                region,
            } => {
                write!(f, "{}", iso.to_name())?;
                let qualifiers: Vec<&str> = script
                    .iter()
                    .chain(region.iter())
                    .map(String::as_str)
                    .collect();
                if !qualifiers.is_empty() {
                    write!(f, " ({})", qualifiers.join("-"))?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tagged(code: &str) -> Language {
        Language::from_code(code).expect("should parse")
    }

    #[test]
    fn parses_639_1_and_639_3() {
        assert_eq!(tagged("en"), tagged("eng"));
        assert_eq!(tagged("ja"), tagged("jpn"));
    }

    #[test]
    fn case_and_separator_do_not_matter() {
        assert_eq!(tagged("PT-br"), tagged("pt_BR"));
        assert_eq!(Language::from_code("ALL").unwrap(), Language::All);
    }

    #[test]
    fn keeps_script_and_region() {
        assert_eq!(tagged("zh-Hans").to_code(), "zh-Hans");
        assert_eq!(tagged("pt-br").to_code(), "pt-BR");
        assert_eq!(tagged("zh-hant-hk").to_code(), "zh-Hant-HK");
    }

    #[test]
    fn pt_br_is_not_pt() {
        assert_ne!(tagged("pt-BR"), tagged("pt"));
        assert_ne!(tagged("zh-Hans"), tagged("zh-Hant"));
    }

    #[test]
    fn round_trips() {
        for code in ["en", "pt-BR", "zh-Hans", "all"] {
            assert_eq!(tagged(code).to_code(), code);
        }
    }

    #[test]
    fn rejects_junk() {
        assert!(Language::from_code("").is_err());
        assert!(Language::from_code("zzz").is_err());
    }

    #[test]
    fn any_language_spellings_collapse() {
        for code in ["all", "mul", "mult", "multi", "MULTI", " all "] {
            assert_eq!(Language::from_code(code).unwrap(), Language::All, "{code}");
        }
    }

    #[test]
    fn all_is_not_a_tagged_language() {
        assert_ne!(Language::All, tagged("en"));
        // `mul` is a real 639-3 code ("Multiple languages") and used to end up
        // tagged instead of All.
        assert_eq!(Language::from_code("mul").unwrap().to_code(), "all");
    }
}
